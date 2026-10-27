//! The catalog guard authorizes the requests that name their targets only in
//! the body: a rename, by its source and its destination, and a multi-table
//! commit, by every table it changes; and the S3 signer gives no signature
//! (spec `docs/en/specs/2026-10-27-catalog-rename-authorizes-source-design.md`).
//!
//! Every test drives the REAL [`EgretCatalogGuard`] in front of lakekeeper's
//! real Iceberg router, wired as `serve` wires it (`mount_catalog_engine`: the
//! guard, the view hold, the table hook), on an in-memory warehouse. Tables
//! and views are made through lakekeeper with no guard in front.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use super::{EgretCatalogGuard, CATALOG_AUDIENCE};
use axum::body::Body;
use axum::http::StatusCode;
use egret_catalog_engine::{
    build_state, catalog_engine_router, create_memory_warehouse, ensure_bootstrapped,
    front_catalog_router, mount_catalog_engine, run_lakekeeper_migrations, GrantInput, TableHook,
    ViewHold,
};
use egret_machine_idp::SigningKeyStore as _;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt as _;

const SYSTEM_USER: &str = "egret-system";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    )
}

/// The message of a refusal: the guard answers in the Iceberg error model
/// (`{"error": {"message": ..}}`).
fn message(body: &Value) -> &str {
    body["error"]["message"].as_str().unwrap_or("")
}

static LAKEKEEPER: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Held across a warehouse's create and its profile write ([`env_with_profile`]).
static CREATING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The view hold's EXPLAIN never runs here: no view in these tests carries
/// lineage or a classification.
struct NoExplain;

#[async_trait::async_trait]
impl egret_perm_compile::lineage::ExplainRunner for NoExplain {
    async fn explain_io(&self, _: &str, _: &str, _: &str) -> Result<Value, String> {
        Err("no EXPLAIN in the rename guard tests".into())
    }
}

struct Env {
    /// The guard, the view hold and the table hook in front of lakekeeper.
    app: axum::Router,
    /// Lakekeeper with nothing in front.
    raw: axum::Router,
    keys: egret_machine_idp::PgSigningKeyStore,
    catalog: egret_catalog_engine::CatalogStore,
    prefix: String,
    warehouse: String,
    warehouse_id: uuid::Uuid,
}

/// A warehouse with namespaces `sales`, `stage`, `corp` and `corp.fin`.
async fn env() -> Option<Env> {
    env_with_profile(None).await
}

/// The same, or, with `profile`, a warehouse whose lakekeeper storage profile
/// is `profile` from the start (no namespaces: nothing touches its storage).
/// The profile is written before anything loads the warehouse, so
/// lakekeeper's warehouse cache never holds the in-memory one.
async fn env_with_profile(profile: Option<Value>) -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = egret_metastore::Db::connect(&dsn, 10)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    LAKEKEEPER
        .get_or_init(|| async {
            run_lakekeeper_migrations(&dsn)
                .await
                .expect("lakekeeper migrations");
            let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
            ensure_bootstrapped(state, SYSTEM_USER)
                .await
                .expect("bootstrap");
        })
        .await;
    let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let pool = &db.pool;
    // lakekeeper's create lists the project's warehouses and caches each one
    // (its location-overlap check), so another test's create between this
    // create and the profile write would cache the in-memory profile. The
    // lock keeps the two together.
    let creating = CREATING.lock().await;
    let prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("rnwh"))
        .await
        .expect("lakekeeper warehouse");
    if let Some(profile) = &profile {
        sqlx::query(
            "UPDATE lakekeeper.warehouse SET storage_profile = $2 WHERE warehouse_id = $1::uuid",
        )
        .bind(&prefix)
        .bind(profile)
        .execute(pool)
        .await
        .expect("storage profile");
    }
    drop(creating);
    let ws: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'RN') RETURNING id")
            .bind(unique("rn-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(unique("rn-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse = unique("rn_wh");
    let warehouse_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(&warehouse)
    .bind(ws)
    .bind(sc)
    .bind(&prefix)
    .fetch_one(pool)
    .await
    .expect("warehouse");

    let guard: Arc<dyn egret_catalog_engine::CatalogGuard> = Arc::new(EgretCatalogGuard {
        keys: egret_machine_idp::PgSigningKeyStore::new(db.clone()),
        catalog: egret_catalog_engine::CatalogStore::new(db.clone()),
    });
    let hold = Arc::new(ViewHold::new(db.clone(), Arc::new(NoExplain)));
    let hook = Arc::new(TableHook::new(db.clone()));
    let env = Env {
        app: mount_catalog_engine(state.clone(), Some(guard), Some(hold), Some(hook)),
        raw: front_catalog_router(catalog_engine_router(state), None, None),
        keys: egret_machine_idp::PgSigningKeyStore::new(db.clone()),
        catalog: egret_catalog_engine::CatalogStore::new(db),
        prefix,
        warehouse,
        warehouse_id,
    };
    if profile.is_some() {
        return Some(env);
    }
    for ns in [
        json!(["sales"]),
        json!(["stage"]),
        json!(["corp"]),
        json!(["corp", "fin"]),
    ] {
        let (status, body) = env
            .send(
                &env.raw,
                None,
                "POST",
                "namespaces",
                json!({ "namespace": ns }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "create namespace {ns}: {body}");
    }
    Some(env)
}

/// One grant of a role.
struct Grant<'a> {
    securable: &'a str,
    namespace: Option<&'a [&'a str]>,
    table: Option<&'a str>,
    privilege: &'a str,
}

fn grant<'a>(
    securable: &'a str,
    namespace: Option<&'a [&'a str]>,
    table: Option<&'a str>,
    privilege: &'a str,
) -> Grant<'a> {
    Grant {
        securable,
        namespace,
        table,
        privilege,
    }
}

impl Env {
    async fn send(
        &self,
        app: &axum::Router,
        token: Option<&str>,
        method: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        self.send_bytes(app, token, method, path, body.to_string().into_bytes())
            .await
    }

    async fn send_bytes(
        &self,
        app: &axum::Router,
        token: Option<&str>,
        method: &str,
        path: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder()
            .method(method)
            .uri(format!("/{}/{path}", self.prefix))
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let body = if method == "GET" {
            Body::empty()
        } else {
            Body::from(body)
        };
        let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, value)
    }

    /// A token for a fresh role holding exactly `grants`.
    async fn token_for(&self, consumer: &str, grants: &[Grant<'_>]) -> String {
        let role_name = unique("rn-role");
        let role = self
            .catalog
            .create_role(self.warehouse_id, &role_name, consumer, None)
            .await
            .expect("role");
        for g in grants {
            self.catalog
                .add_grant(
                    role.id,
                    &GrantInput {
                        securable_type: g.securable.into(),
                        namespace: g
                            .namespace
                            .map(|n| n.iter().map(|s| s.to_string()).collect()),
                        table_name: g.table.map(str::to_string),
                        privilege: g.privilege.into(),
                    },
                )
                .await
                .expect("grant");
        }
        let km = self.keys.active().await.expect("active key");
        egret_machine_idp::sign_claims(
            &km,
            &json!({
                "sub": unique("rn-client"), "aud": CATALOG_AUDIENCE,
                "warehouse": self.warehouse, "roles": [role_name],
                "exp": chrono::Utc::now().timestamp() + 600,
            }),
        )
        .expect("sign")
    }

    async fn create_table(&self, ns: &str, name: &str) {
        let schema = json!({"type": "struct", "schema-id": 0, "fields": [
            {"id": 1, "name": "id", "type": "string", "required": false}
        ]});
        let (status, body) = self
            .send(
                &self.raw,
                None,
                "POST",
                &format!("namespaces/{ns}/tables"),
                json!({"name": name, "schema": schema}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "create {ns}.{name}: {body}");
    }

    async fn create_view(&self, ns: &str, name: &str) {
        let body = json!({
            "name": name,
            "schema": {"type": "struct", "schema-id": 0, "fields": [
                {"id": 1, "name": "id", "type": "string", "required": false}
            ]},
            "view-version": {
                "version-id": 1,
                "schema-id": 0,
                "timestamp-ms": chrono::Utc::now().timestamp_millis(),
                "summary": {"engine-name": "trino", "engine-version": "483"},
                "default-namespace": [ns],
                "representations": [{"type": "sql", "sql": "SELECT 1 AS id", "dialect": "trino"}]
            },
            "properties": {}
        });
        let (status, body) = self
            .send(
                &self.raw,
                None,
                "POST",
                &format!("namespaces/{ns}/views"),
                body,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "create view {ns}.{name}: {body}");
    }

    /// Does lakekeeper hold `{ns}/{kind}/{name}` (`kind` is `tables` or `views`)?
    async fn exists(&self, kind: &str, ns: &str, name: &str) -> bool {
        let (status, body) = self
            .send(
                &self.raw,
                None,
                "GET",
                &format!("namespaces/{ns}/{kind}/{name}"),
                Value::Null,
            )
            .await;
        match status {
            StatusCode::OK => true,
            StatusCode::NOT_FOUND => false,
            other => panic!("load {ns}/{kind}/{name}: {other} {body}"),
        }
    }

    async fn rename(
        &self,
        token: &str,
        kind: &str,
        source: (&[&str], &str),
        destination: (&[&str], &str),
    ) -> (StatusCode, Value) {
        self.send(
            &self.app,
            Some(token),
            "POST",
            &format!("{kind}/rename"),
            json!({
                "source": {"namespace": source.0, "name": source.1},
                "destination": {"namespace": destination.0, "name": destination.1},
            }),
        )
        .await
    }

    async fn load(&self, token: &str, kind: &str, ns: &str, name: &str) -> StatusCode {
        self.send(
            &self.app,
            Some(token),
            "GET",
            &format!("namespaces/{ns}/{kind}/{name}"),
            Value::Null,
        )
        .await
        .0
    }
}

macro_rules! env_or_skip {
    () => {
        match env().await {
            Some(e) => e,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

/// F-A, the premise: a table grant on a name that does not exist yet is
/// enough to rename a table the role cannot read onto that name, and then
/// read it.
#[tokio::test]
async fn a_table_grant_on_the_destination_does_not_rename_an_unreadable_table() {
    let env = env_or_skip!();
    env.create_table("sales", "secret").await;
    let token = env
        .token_for(
            "external",
            &[grant("table", Some(&["sales"]), Some("loot"), "write")],
        )
        .await;
    assert_eq!(
        env.load(&token, "tables", "sales", "secret").await,
        StatusCode::FORBIDDEN,
        "the role cannot read the source"
    );

    let (status, body) = env
        .rename(
            &token,
            "tables",
            (&["sales"], "secret"),
            (&["sales"], "loot"),
        )
        .await;
    let read = env.load(&token, "tables", "sales", "loot").await;
    assert_eq!(
        (status, read),
        (StatusCode::FORBIDDEN, StatusCode::NOT_FOUND),
        "rename answered {status} {body}, then the read under the new name answered {read}"
    );
    assert!(
        message(&body).contains("sales.secret"),
        "the 403 names the source: {body}"
    );
    assert!(env.exists("tables", "sales", "secret").await, "not renamed");
}

/// F-A: a write grant on another namespace is enough to move an unreadable
/// table into it, and then read it.
#[tokio::test]
async fn a_write_grant_elsewhere_does_not_move_an_unreadable_table_into_it() {
    let env = env_or_skip!();
    env.create_table("sales", "secret").await;
    let token = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["stage"]), None, "write")],
        )
        .await;

    let (status, body) = env
        .rename(
            &token,
            "tables",
            (&["sales"], "secret"),
            (&["stage"], "secret"),
        )
        .await;
    let read = env.load(&token, "tables", "stage", "secret").await;
    assert_eq!(
        (status, read),
        (StatusCode::FORBIDDEN, StatusCode::NOT_FOUND),
        "rename answered {status} {body}, then the read under the new name answered {read}"
    );
    assert!(env.exists("tables", "sales", "secret").await, "not renamed");
}

/// A read grant on the source is not a write: a rename removes the source
/// name, as a drop does.
#[tokio::test]
async fn a_read_grant_on_the_source_is_not_enough() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    let token = env
        .token_for(
            "external",
            &[
                grant("table", Some(&["sales"]), Some("orders"), "read"),
                grant("namespace", Some(&["stage"]), None, "write"),
            ],
        )
        .await;
    let (status, body) = env
        .rename(
            &token,
            "tables",
            (&["sales"], "orders"),
            (&["stage"], "orders"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(env.exists("tables", "sales", "orders").await, "not renamed");
}

/// The destination: write on the source does not create a table in a
/// namespace the role may not create tables in.
#[tokio::test]
async fn an_unauthorized_destination_is_refused() {
    let env = env_or_skip!();
    env.create_table("sales", "mine").await;
    let token = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["sales"]), None, "write")],
        )
        .await;
    let (status, body) = env
        .rename(&token, "tables", (&["sales"], "mine"), (&["stage"], "mine"))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        message(&body).contains("stage"),
        "the 403 names the destination namespace: {body}"
    );
    assert!(env.exists("tables", "sales", "mine").await, "not renamed");
    assert!(!env.exists("tables", "stage", "mine").await, "not created");
}

/// Controller default D2: the destination needs the privilege a create in
/// its namespace needs. A table grant on the destination name is not one,
/// even with write on the source.
#[tokio::test]
async fn a_table_grant_on_the_destination_name_does_not_create_it() {
    let env = env_or_skip!();
    env.create_table("sales", "mine").await;
    let token = env
        .token_for(
            "external",
            &[
                grant("table", Some(&["sales"]), Some("mine"), "write"),
                grant("table", Some(&["sales"]), Some("mine2"), "write"),
            ],
        )
        .await;
    let (status, body) = env
        .rename(
            &token,
            "tables",
            (&["sales"], "mine"),
            (&["sales"], "mine2"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(env.exists("tables", "sales", "mine").await, "not renamed");
}

/// Write on the source's namespace and on the destination's renames, across
/// namespaces, and the table hook (#757) still lets the rename through.
#[tokio::test]
async fn an_authorized_rename_still_renames() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    let token = env
        .token_for(
            "external",
            &[
                grant("table", Some(&["sales"]), Some("orders"), "write"),
                grant("namespace", Some(&["stage"]), None, "manage"),
            ],
        )
        .await;
    let (status, body) = env
        .rename(
            &token,
            "tables",
            (&["sales"], "orders"),
            (&["stage"], "orders_v2"),
        )
        .await;
    assert!(status.is_success(), "rename: {status} {body}");
    assert!(env.exists("tables", "stage", "orders_v2").await);
    assert!(!env.exists("tables", "sales", "orders").await);
}

/// An engine's catalog client holds `manage` on the whole warehouse
/// (`egret_engine_mgmt::engines`); it renames whatever it likes.
#[tokio::test]
async fn an_engine_client_with_warehouse_manage_still_renames() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    env.create_view("sales", "v_orders").await;
    let token = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    let (status, body) = env
        .rename(
            &token,
            "tables",
            (&["sales"], "orders"),
            (&["stage"], "orders"),
        )
        .await;
    assert!(status.is_success(), "table rename: {status} {body}");
    assert!(env.exists("tables", "stage", "orders").await);
    let (status, body) = env
        .rename(
            &token,
            "views",
            (&["sales"], "v_orders"),
            (&["stage"], "v_orders"),
        )
        .await;
    assert!(status.is_success(), "view rename: {status} {body}");
    assert!(env.exists("views", "stage", "v_orders").await);
}

/// F-C: a view rename is authorized the same way.
#[tokio::test]
async fn a_view_rename_is_authorized_by_its_source_and_destination() {
    let env = env_or_skip!();
    env.create_view("sales", "v_secret").await;
    env.create_view("sales", "v_mine").await;
    let attacker = env
        .token_for(
            "external",
            &[grant("table", Some(&["sales"]), Some("v_loot"), "write")],
        )
        .await;
    let (status, body) = env
        .rename(
            &attacker,
            "views",
            (&["sales"], "v_secret"),
            (&["sales"], "v_loot"),
        )
        .await;
    let read = env.load(&attacker, "views", "sales", "v_loot").await;
    assert_eq!(
        (status, read),
        (StatusCode::FORBIDDEN, StatusCode::NOT_FOUND),
        "view rename answered {status} {body}, then the read under the new name answered {read}"
    );
    assert!(
        env.exists("views", "sales", "v_secret").await,
        "not renamed"
    );

    let owner = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["sales"]), None, "write")],
        )
        .await;
    let (status, body) = env
        .rename(
            &owner,
            "views",
            (&["sales"], "v_mine"),
            (&["stage"], "v_mine"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "destination: {body}");
    let (status, body) = env
        .rename(
            &owner,
            "views",
            (&["sales"], "v_mine"),
            (&["sales"], "v_mine2"),
        )
        .await;
    assert!(
        status.is_success(),
        "authorized view rename: {status} {body}"
    );
    assert!(env.exists("views", "sales", "v_mine2").await);
}

/// A multi-level namespace matches a grant level by level, exactly: a grant
/// on `corp` does not reach `corp.fin`.
#[tokio::test]
async fn a_multi_level_namespace_matches_exactly() {
    let env = env_or_skip!();
    env.create_table("corp%1Ffin", "ledger").await;
    let parent = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["corp"]), None, "write")],
        )
        .await;
    let (status, body) = env
        .rename(
            &parent,
            "tables",
            (&["corp", "fin"], "ledger"),
            (&["corp"], "ledger"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(env.exists("tables", "corp%1Ffin", "ledger").await);

    let fin = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["corp", "fin"]), None, "write")],
        )
        .await;
    let (status, body) = env
        .rename(
            &fin,
            "tables",
            (&["corp", "fin"], "ledger"),
            (&["corp", "fin"], "ledger_v2"),
        )
        .await;
    assert!(status.is_success(), "{status} {body}");
    assert!(env.exists("tables", "corp%1Ffin", "ledger_v2").await);
}

/// The guard answers a body it cannot read as a rename with its own 400,
/// before lakekeeper sees it.
#[tokio::test]
async fn a_malformed_rename_body_is_a_400_from_the_guard() {
    let env = env_or_skip!();
    let token = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    for (what, body) in [
        ("not JSON", b"{not json".to_vec()),
        (
            "no destination",
            json!({"source": {"namespace": ["sales"], "name": "t"}})
                .to_string()
                .into_bytes(),
        ),
        (
            "an empty namespace",
            json!({
                "source": {"namespace": [], "name": "t"},
                "destination": {"namespace": ["sales"], "name": "u"},
            })
            .to_string()
            .into_bytes(),
        ),
        (
            "a namespace that is not a list of strings",
            json!({
                "source": {"namespace": "sales", "name": "t"},
                "destination": {"namespace": ["sales"], "name": "u"},
            })
            .to_string()
            .into_bytes(),
        ),
        (
            "an empty name",
            json!({
                "source": {"namespace": ["sales"], "name": ""},
                "destination": {"namespace": ["sales"], "name": "u"},
            })
            .to_string()
            .into_bytes(),
        ),
    ] {
        for kind in ["tables", "views"] {
            let (status, resp) = env
                .send_bytes(
                    &env.app,
                    Some(&token),
                    "POST",
                    &format!("{kind}/rename"),
                    body.clone(),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{kind}, {what}: {resp}");
            assert!(
                message(&resp).starts_with("malformed rename request"),
                "{kind}, {what}: the guard's own 400, not lakekeeper's: {resp}"
            );
        }
    }
}

// ── Multi-table commit (spec §8) ────────────────────────────────────────────

/// One `table-changes` entry that sets a property on `ns.name`.
fn set_property(ns: &[&str], name: &str, value: &str) -> Value {
    json!({
        "identifier": {"namespace": ns, "name": name},
        "requirements": [],
        "updates": [{"action": "set-properties", "updates": {"egret.probe": value}}],
    })
}

impl Env {
    async fn commit(&self, token: &str, changes: Vec<Value>) -> (StatusCode, Value) {
        self.send(
            &self.app,
            Some(token),
            "POST",
            "transactions/commit",
            json!({ "table-changes": changes }),
        )
        .await
    }

    /// The `egret.probe` property of `ns.name`, read through lakekeeper.
    async fn probe(&self, ns: &str, name: &str) -> Option<String> {
        let (status, body) = self
            .send(
                &self.raw,
                None,
                "GET",
                &format!("namespaces/{ns}/tables/{name}"),
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "load {ns}.{name}: {body}");
        body["metadata"]["properties"]["egret.probe"]
            .as_str()
            .map(str::to_string)
    }
}

/// The premise: a write grant on one table commits to another table through
/// the multi-table commit.
#[tokio::test]
async fn a_multi_table_commit_to_an_unwritable_table_is_refused() {
    let env = env_or_skip!();
    env.create_table("sales", "mine").await;
    env.create_table("sales", "theirs").await;
    let token = env
        .token_for(
            "external",
            &[grant("table", Some(&["sales"]), Some("mine"), "write")],
        )
        .await;

    let (status, body) = env
        .commit(&token, vec![set_property(&["sales"], "theirs", "pwned")])
        .await;
    let theirs = env.probe("sales", "theirs").await;
    assert_eq!(
        (status, theirs.as_deref()),
        (StatusCode::FORBIDDEN, None),
        "the commit answered {status} {body}, and left theirs' property {theirs:?}"
    );
    assert!(
        message(&body).contains("sales.theirs"),
        "the 403 names the table: {body}"
    );
}

/// One unwritable table refuses the whole commit: nothing is forwarded, so
/// the writable table in the same commit is unchanged too.
#[tokio::test]
async fn one_unwritable_table_refuses_the_whole_commit() {
    let env = env_or_skip!();
    env.create_table("sales", "mine").await;
    env.create_table("sales", "theirs").await;
    let token = env
        .token_for(
            "external",
            &[
                grant("table", Some(&["sales"]), Some("mine"), "write"),
                grant("table", Some(&["sales"]), Some("theirs"), "read"),
            ],
        )
        .await;
    let (status, body) = env
        .commit(
            &token,
            vec![
                set_property(&["sales"], "mine", "1"),
                set_property(&["sales"], "theirs", "1"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        message(&body).contains("sales.theirs"),
        "the 403 names the refused table: {body}"
    );
    assert_eq!(env.probe("sales", "mine").await, None, "nothing forwarded");
    assert_eq!(env.probe("sales", "theirs").await, None);
}

/// Write on every table commits; an engine with warehouse manage commits
/// anywhere, multi-level namespaces included.
#[tokio::test]
async fn an_authorized_multi_table_commit_still_commits() {
    let env = env_or_skip!();
    env.create_table("sales", "mine").await;
    env.create_table("stage", "other").await;
    env.create_table("corp%1Ffin", "ledger").await;
    let token = env
        .token_for(
            "external",
            &[
                grant("table", Some(&["sales"]), Some("mine"), "write"),
                grant("namespace", Some(&["stage"]), None, "manage"),
            ],
        )
        .await;
    let (status, body) = env
        .commit(
            &token,
            vec![
                set_property(&["sales"], "mine", "a"),
                set_property(&["stage"], "other", "a"),
            ],
        )
        .await;
    assert!(status.is_success(), "commit: {status} {body}");
    assert_eq!(env.probe("sales", "mine").await.as_deref(), Some("a"));
    assert_eq!(env.probe("stage", "other").await.as_deref(), Some("a"));

    let engine = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    let (status, body) = env
        .commit(
            &engine,
            vec![
                set_property(&["sales"], "mine", "b"),
                set_property(&["corp", "fin"], "ledger", "b"),
            ],
        )
        .await;
    assert!(status.is_success(), "engine commit: {status} {body}");
    assert_eq!(
        env.probe("corp%1Ffin", "ledger").await.as_deref(),
        Some("b")
    );
}

/// A grant on `corp` does not reach a table in `corp.fin`.
#[tokio::test]
async fn a_multi_table_commit_matches_namespaces_exactly() {
    let env = env_or_skip!();
    env.create_table("corp%1Ffin", "ledger").await;
    let token = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["corp"]), None, "write")],
        )
        .await;
    let (status, body) = env
        .commit(&token, vec![set_property(&["corp", "fin"], "ledger", "x")])
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(env.probe("corp%1Ffin", "ledger").await, None);
}

/// A body the guard cannot read as a multi-table commit is its own 400.
#[tokio::test]
async fn a_malformed_multi_table_commit_is_a_400_from_the_guard() {
    let env = env_or_skip!();
    let token = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    for (what, body) in [
        ("not JSON", b"{not json".to_vec()),
        ("no table-changes", b"{}".to_vec()),
        (
            "table-changes not a list",
            json!({"table-changes": {}}).to_string().into_bytes(),
        ),
        (
            "an empty list",
            json!({"table-changes": []}).to_string().into_bytes(),
        ),
        (
            "a change with no identifier",
            json!({"table-changes": [{"requirements": [], "updates": []}]})
                .to_string()
                .into_bytes(),
        ),
        (
            "an empty namespace",
            json!({"table-changes": [set_property(&[], "t", "x")]})
                .to_string()
                .into_bytes(),
        ),
    ] {
        let (status, resp) = env
            .send_bytes(&env.app, Some(&token), "POST", "transactions/commit", body)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {resp}");
        assert!(
            message(&resp).starts_with("malformed multi-table commit"),
            "{what}: the guard's own 400, not lakekeeper's: {resp}"
        );
    }
}

// ── S3 signer (spec §9, §10.6) ──────────────────────────────────────────────

/// A sign request for `method` on a file under `egret-sign/kp`.
fn sign_request(method: &str) -> Value {
    json!({
        "region": "us-east-1",
        "uri": "http://127.0.0.1:1/egret-sign/kp/some-table/data/x.parquet",
        "method": method,
        "headers": {},
    })
}

/// egret's S3 warehouse profile, on a bucket nothing reaches.
fn egret_s3_profile() -> Value {
    egret_catalog_engine::s3_storage_profile(&egret_catalog_engine::S3StorageConfig {
        bucket: "egret-sign".into(),
        region: "us-east-1".into(),
        endpoint: Some("http://127.0.0.1:1/".into()),
        path_style: true,
        access_key: "AK".into(),
        secret_key: "SK".into(),
        key_prefix: Some("kp".into()),
    })
}

/// Behind the guard's own refusal (below), lakekeeper still refuses to sign on
/// a warehouse with egret's S3 profile: remote signing is off, and it refuses
/// before it maps the location to a table. Lakekeeper's own table check runs
/// on `AllowAllAuthorizer`, so this refusal is pinned, straight against
/// lakekeeper's router, for a GET and a PUT.
#[tokio::test]
async fn lakekeeper_refuses_to_sign_on_an_egret_warehouse() {
    let Some(env) = env_with_profile(Some(egret_s3_profile())).await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    for method in ["GET", "PUT"] {
        let (status, body) = env
            .send(
                &env.raw,
                None,
                "POST",
                "v1/aws/s3/sign",
                sign_request(method),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}: {body}");
        assert_eq!(
            body["error"]["type"], "RemoteSigningDisabled",
            "{method}: lakekeeper's refusal, before any table lookup: {body}"
        );
    }
}

/// F6: the guard refuses every signer route itself, whatever the caller's
/// grants, so the signer is closed even if a profile ever turns signing on.
#[tokio::test]
async fn the_guard_refuses_remote_signing_outright() {
    let Some(env) = env_with_profile(Some(egret_s3_profile())).await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let engine = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    let paths = [
        format!("/{}/v1/aws/s3/sign", env.prefix),
        format!(
            "/signer/{}/tabular-id/{}/v1/aws/s3/sign",
            env.prefix,
            uuid::Uuid::new_v4()
        ),
        "/aws/s3/sign".to_string(),
    ];
    for path in &paths {
        for method in ["GET", "PUT"] {
            let (status, body) = env
                .send_abs(
                    Some(&engine),
                    path,
                    Body::from(sign_request(method).to_string()),
                    &[],
                )
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path} {method}: {body}");
            assert!(
                message(&body).contains("remote signing is not used by egret"),
                "{path} {method}: the guard's own refusal: {body}"
            );
        }
    }
}

// ── The review's findings (spec §10) ────────────────────────────────────────

impl Env {
    /// Send to an absolute, lakekeeper-relative `path`, with `headers`; the
    /// answer must come within 5 s (a hang is a panic, not a timeout of the
    /// test run).
    async fn send_abs(
        &self,
        token: Option<&str>,
        path: &str,
        body: Body,
        headers: &[(&str, String)],
    ) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.app.clone().oneshot(req.body(body).unwrap()),
        )
        .await
        .unwrap_or_else(|_| panic!("{path}: no answer within 5 s"))
        .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, value)
    }
}

/// F8: a body with a duplicate key is never decided on one copy and run on
/// the other: whichever copy is unauthorized, nothing is renamed.
#[tokio::test]
async fn a_duplicate_key_body_renames_nothing() {
    let env = env_or_skip!();
    env.create_table("sales", "secret").await;
    env.create_table("stage", "mine").await;
    let token = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["stage"]), None, "write")],
        )
        .await;
    let secret = r#"{"namespace":["sales"],"name":"secret"}"#;
    let mine = r#"{"namespace":["stage"],"name":"mine"}"#;
    let dst = r#"{"namespace":["stage"],"name":"loot"}"#;
    for (first, last) in [(secret, mine), (mine, secret)] {
        let body = format!(r#"{{"source":{first},"source":{last},"destination":{dst}}}"#);
        let (status, resp) = env
            .send_bytes(
                &env.app,
                Some(&token),
                "POST",
                "tables/rename",
                body.into_bytes(),
            )
            .await;
        assert!(!status.is_success(), "{status} {resp}");
        assert!(env.exists("tables", "sales", "secret").await);
        assert!(env.exists("tables", "stage", "mine").await);
        assert!(!env.exists("tables", "stage", "loot").await);
    }
}
/// A request body that yields `first` and then never ends, counting the bytes
/// the server pulled from it.
fn endless_body(first: usize) -> (Body, Arc<std::sync::atomic::AtomicUsize>) {
    use futures_util::StreamExt as _;
    let pulled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = pulled.clone();
    let stream = futures_util::stream::once(async move {
        Ok::<_, std::io::Error>(bytes::Bytes::from(vec![b' '; first]))
    })
    .chain(futures_util::stream::pending())
    .inspect(move |chunk| {
        if let Ok(c) = chunk {
            counter.fetch_add(c.len(), std::sync::atomic::Ordering::SeqCst);
        }
    });
    (Body::from_stream(stream), pulled)
}

/// A body of `chunks` chunks of 1 MiB of spaces, with no length declared.
fn streamed_body(chunks: usize) -> Body {
    let stream = futures_util::stream::iter(
        (0..chunks).map(|_| Ok::<_, std::io::Error>(bytes::Bytes::from(vec![b' '; 1 << 20]))),
    );
    Body::from_stream(stream)
}

/// F2: an unauthenticated request is answered 401 before its body is read,
/// so it cannot make the mount buffer anything.
#[tokio::test]
async fn an_unauthenticated_body_is_not_read() {
    let env = env_or_skip!();
    for route in [
        "transactions/commit",
        "tables/rename",
        "namespaces/sales/tables/t",
    ] {
        let (body, pulled) = endless_body(64 << 10);
        let (status, resp) = env
            .send_abs(None, &format!("/{}/{route}", env.prefix), body, &[])
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{route}: {resp}");
        assert_eq!(
            pulled.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{route}: nothing of the body was read"
        );
    }
}

/// F2: a declared length over the cap is a 413 before the body is read.
#[tokio::test]
async fn a_declared_length_over_the_cap_is_a_413_unread() {
    let env = env_or_skip!();
    let token = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    for (route, cap) in [
        ("transactions/commit", 16usize << 20),
        ("tables/rename", 1 << 20),
    ] {
        let (body, pulled) = endless_body(1024);
        let (status, resp) = env
            .send_abs(
                Some(&token),
                &format!("/{}/{route}", env.prefix),
                body,
                &[("content-length", (cap + 1).to_string())],
            )
            .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{route}: {resp}");
        assert_eq!(
            pulled.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{route}: nothing of the body was read"
        );
    }
}

/// F2: a streamed body that crosses the cap is a 413.
#[tokio::test]
async fn a_streamed_body_over_the_cap_is_a_413() {
    let env = env_or_skip!();
    let token = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    for (route, chunks) in [("transactions/commit", 17), ("tables/rename", 2)] {
        let (status, resp) = env
            .send_abs(
                Some(&token),
                &format!("/{}/{route}", env.prefix),
                streamed_body(chunks),
                &[],
            )
            .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{route}: {resp}");
    }
}

/// A single-table commit with `body`, through the guard.
impl Env {
    async fn commit_one(
        &self,
        token: &str,
        kind: &str,
        path_ns: &str,
        path_name: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        self.send(
            &self.app,
            Some(token),
            "POST",
            &format!("namespaces/{path_ns}/{kind}/{path_name}"),
            body,
        )
        .await
    }

    /// The `egret.probe` property of view `ns.name`, read through lakekeeper.
    async fn view_probe(&self, ns: &str, name: &str) -> Option<String> {
        let (status, body) = self
            .send(
                &self.raw,
                None,
                "GET",
                &format!("namespaces/{ns}/views/{name}"),
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "load view {ns}.{name}: {body}");
        body["metadata"]["properties"]["egret.probe"]
            .as_str()
            .map(str::to_string)
    }
}

/// A commit body that sets `egret.probe`, naming `identifier` when given.
fn commit_body(identifier: Option<(&[&str], &str)>, value: &str) -> Value {
    let mut body = json!({
        "requirements": [],
        "updates": [{"action": "set-properties", "updates": {"egret.probe": value}}],
    });
    if let Some((ns, name)) = identifier {
        body["identifier"] = json!({"namespace": ns, "name": name});
    }
    body
}

/// F1, the review's scenario: a grant on namespace `corp.fin` must not commit
/// to the TABLE `corp.fin` by naming it in a single-table commit's body
/// (lakekeeper's "Spark branch" fallback commits to the body's table).
#[tokio::test]
async fn a_single_table_commit_cannot_reach_the_table_its_body_names() {
    let env = env_or_skip!();
    env.create_table("corp", "fin").await;
    let team = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["corp", "fin"]), None, "write")],
        )
        .await;
    let (status, body) = env
        .commit_one(
            &team,
            "tables",
            "corp%1Ffin",
            "anything",
            commit_body(Some((&["corp"], "fin")), "pwned"),
        )
        .await;
    let probe = env.probe("corp", "fin").await;
    assert_eq!(
        (status, probe.as_deref()),
        (StatusCode::FORBIDDEN, None),
        "the commit answered {status} {body}, and left corp.fin's property {probe:?}"
    );
}

/// F1 on views: lakekeeper commits a view to the view in the PATH and only
/// validates the body's `identifier` (`views/commit.rs`: `determine_table_ident`
/// at :61, then `load_and_authorize_view_operation` on `parameters.view`), so
/// the review's scenario does not reach the view the body names. Pinned: a
/// namespace grant on `corp.vfin` changes nothing on the view `corp.vfin`.
#[tokio::test]
async fn a_view_commit_reaches_only_the_view_in_its_path() {
    let env = env_or_skip!();
    env.create_view("corp", "vfin").await;
    // The path's namespace must exist for lakekeeper to get past loading it.
    let (status, body) = env
        .send(
            &env.raw,
            None,
            "POST",
            "namespaces",
            json!({"namespace": ["corp", "vfin"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "namespace corp.vfin: {body}");
    let team = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["corp", "vfin"]), None, "write")],
        )
        .await;
    let (status, body) = env
        .commit_one(
            &team,
            "views",
            "corp%1Fvfin",
            "anything",
            commit_body(Some((&["corp"], "vfin")), "pwned"),
        )
        .await;
    assert!(!status.is_success(), "{status} {body}");
    assert_eq!(env.view_probe("corp", "vfin").await, None);
}

/// F1: a real Spark branch commit (path `corp.fin/branch_x`, body `corp.fin`)
/// is authorized by the table it commits to, so a grant on that table
/// commits; a commit whose body names its own path still commits.
#[tokio::test]
async fn a_spark_branch_commit_is_authorized_by_the_table_it_commits_to() {
    let env = env_or_skip!();
    env.create_table("corp", "fin").await;
    let owner = env
        .token_for(
            "external",
            &[grant("table", Some(&["corp"]), Some("fin"), "write")],
        )
        .await;
    let (status, body) = env
        .commit_one(
            &owner,
            "tables",
            "corp%1Ffin",
            "branch_x",
            commit_body(Some((&["corp"], "fin")), "branch"),
        )
        .await;
    assert!(status.is_success(), "spark branch commit: {status} {body}");
    assert_eq!(env.probe("corp", "fin").await.as_deref(), Some("branch"));

    let (status, body) = env
        .commit_one(
            &owner,
            "tables",
            "corp",
            "fin",
            commit_body(Some((&["corp"], "fin")), "plain"),
        )
        .await;
    assert!(status.is_success(), "plain commit: {status} {body}");
    let (status, body) = env
        .commit_one(&owner, "tables", "corp", "fin", commit_body(None, "bare"))
        .await;
    assert!(
        status.is_success(),
        "commit with no identifier: {status} {body}"
    );
    assert_eq!(env.probe("corp", "fin").await.as_deref(), Some("bare"));
}

/// F1: a single-table commit body the guard cannot read is its own 400.
#[tokio::test]
async fn a_malformed_single_table_commit_is_a_400_from_the_guard() {
    let env = env_or_skip!();
    env.create_table("sales", "t").await;
    let token = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    for (what, body) in [
        ("not JSON", b"{not json".to_vec()),
        (
            "an identifier with an empty namespace",
            commit_body(Some((&[], "t")), "x").to_string().into_bytes(),
        ),
    ] {
        let (status, resp) = env
            .send_bytes(
                &env.app,
                Some(&token),
                "POST",
                "namespaces/sales/tables/t",
                body,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {resp}");
        assert!(
            message(&resp).starts_with("malformed table commit"),
            "{what}: the guard's own 400: {resp}"
        );
    }
}

/// F3: the guard matches names as lakekeeper does (its ICU `case_insensitive`
/// collation): a grant on `Secret` reaches `secret` however the request spells
/// it, and the reverse. Before, it matched bytes, so the answer depended on
/// how the caller spelled the name.
#[tokio::test]
async fn a_grant_matches_names_as_the_catalog_does() {
    let env = env_or_skip!();
    env.create_table("sales", "secret").await;
    for (grant_ns, grant_name) in [("sales", "Secret"), ("SALES", "secret")] {
        let token = env
            .token_for(
                "external",
                &[grant("table", Some(&[grant_ns]), Some(grant_name), "read")],
            )
            .await;
        for (ns, name) in [
            ("sales", "secret"),
            ("Sales", "SECRET"),
            ("sales", "Secret"),
        ] {
            assert_eq!(
                env.load(&token, "tables", ns, name).await,
                StatusCode::OK,
                "grant {grant_ns}.{grant_name}, request {ns}.{name}"
            );
        }
    }
    // A name that is not the same under the collation is still refused.
    let other = env
        .token_for(
            "external",
            &[grant("table", Some(&["sales"]), Some("secrets"), "read")],
        )
        .await;
    assert_eq!(
        env.load(&other, "tables", "sales", "secret").await,
        StatusCode::FORBIDDEN
    );
}

/// F3: for spellings that differ in case, width, composition or invisible
/// characters, the guard's decision for a grant on `sales.secret` agrees with
/// what lakekeeper resolves: allowed exactly when lakekeeper finds the table.
#[tokio::test]
async fn the_guard_decides_names_the_way_lakekeeper_resolves_them() {
    let env = env_or_skip!();
    env.create_table("sales", "secret").await;
    env.create_table("sales", "caf\u{e9}").await;
    let token = env
        .token_for(
            "external",
            &[
                grant("table", Some(&["sales"]), Some("secret"), "read"),
                grant("table", Some(&["sales"]), Some("caf\u{e9}"), "read"),
            ],
        )
        .await;
    let variants = [
        "secret",
        "SECRET",
        "\u{ff53}\u{ff45}\u{ff43}\u{ff52}\u{ff45}\u{ff54}",
        "sec\u{ad}ret",
        "sec\u{200b}ret",
        "secre",
        "cafe\u{301}",
        "CAF\u{c9}",
        "cafe",
    ];
    for v in variants {
        let path = percent(v);
        let (raw, _) = env
            .send(
                &env.raw,
                None,
                "GET",
                &format!("namespaces/sales/tables/{path}"),
                Value::Null,
            )
            .await;
        let guarded = env.load(&token, "tables", "sales", &path).await;
        let expected = match raw {
            StatusCode::OK => StatusCode::OK,
            StatusCode::NOT_FOUND => StatusCode::FORBIDDEN,
            other => panic!("{v:?}: lakekeeper answered {other}"),
        };
        assert_eq!(
            guarded, expected,
            "{v:?}: lakekeeper answered {raw}, so the guard must answer {expected}"
        );
    }
}

/// Percent-encode every byte that is not ASCII alphanumeric.
fn percent(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// F3 on a body: a rename whose source is spelled in another case is decided
/// on the table lakekeeper renames.
#[tokio::test]
async fn a_rename_source_in_another_case_is_decided_on_the_table() {
    let env = env_or_skip!();
    env.create_table("sales", "secret").await;
    env.create_table("sales", "mine").await;
    let owner = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["SALES"]), None, "write")],
        )
        .await;
    let (status, body) = env
        .rename(
            &owner,
            "tables",
            (&["sales"], "MINE"),
            (&["sales"], "mine2"),
        )
        .await;
    assert!(status.is_success(), "{status} {body}");
    assert!(env.exists("tables", "sales", "mine2").await);
}

/// F5: a recursive namespace drop deletes every child namespace and table, so
/// it needs a catalog-level write; a grant on the parent alone is refused.
#[tokio::test]
async fn a_recursive_namespace_drop_needs_a_catalog_grant() {
    let env = env_or_skip!();
    env.create_table("corp%1Ffin", "ledger").await;
    let parent = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["corp"]), None, "manage")],
        )
        .await;
    for query in [
        "recursive=true",
        "recursiv%65=true",
        "recursive=true&purge=true",
    ] {
        let (status, body) = env
            .send(
                &env.app,
                Some(&parent),
                "DELETE",
                &format!("namespaces/corp?{query}"),
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{query}: {body}");
        assert!(
            env.exists("tables", "corp%1Ffin", "ledger").await,
            "{query}: the child's table survives"
        );
    }

    let engine = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    let (status, body) = env
        .send(
            &env.app,
            Some(&engine),
            "DELETE",
            "namespaces/corp?recursive=true",
            Value::Null,
        )
        .await;
    assert!(
        status.is_success(),
        "catalog manage drops recursively: {status} {body}"
    );
}

/// O2: every refusal by the guard or the mount is in the Iceberg REST error
/// model, `{"error": {"message", "type", "code"}}`, so Iceberg clients parse
/// it: 401, 403, 400 and 413, on guarded and unguarded routes.
#[tokio::test]
async fn refusals_use_the_iceberg_error_model() {
    let env = env_or_skip!();
    env.create_table("sales", "secret").await;
    let nobody = env
        .token_for(
            "external",
            &[grant("namespace", Some(&["stage"]), None, "read")],
        )
        .await;
    let engine = env
        .token_for("engine", &[grant("catalog", None, None, "manage")])
        .await;
    let rename = json!({
        "source": {"namespace": ["sales"], "name": "secret"},
        "destination": {"namespace": ["sales"], "name": "loot"},
    });
    let cases: Vec<(&str, StatusCode, &str, (StatusCode, Value))> = vec![
        (
            "no token",
            StatusCode::UNAUTHORIZED,
            "NotAuthorizedException",
            env.send(&env.app, None, "POST", "tables/rename", rename.clone())
                .await,
        ),
        (
            "rename refused",
            StatusCode::FORBIDDEN,
            "ForbiddenException",
            env.send(&env.app, Some(&nobody), "POST", "tables/rename", rename)
                .await,
        ),
        (
            "load refused",
            StatusCode::FORBIDDEN,
            "ForbiddenException",
            env.send(
                &env.app,
                Some(&nobody),
                "GET",
                "namespaces/sales/tables/secret",
                Value::Null,
            )
            .await,
        ),
        (
            "malformed",
            StatusCode::BAD_REQUEST,
            "BadRequestException",
            env.send_bytes(
                &env.app,
                Some(&engine),
                "POST",
                "transactions/commit",
                b"[1,2".to_vec(),
            )
            .await,
        ),
        (
            "too large",
            StatusCode::PAYLOAD_TOO_LARGE,
            "RequestTooLargeException",
            env.send_abs(
                Some(&engine),
                &format!("/{}/tables/rename", env.prefix),
                streamed_body(2),
                &[],
            )
            .await,
        ),
    ];
    for (what, status, kind, (got, body)) in cases {
        assert_eq!(got, status, "{what}: {body}");
        assert_eq!(body["error"]["type"], kind, "{what}: {body}");
        assert_eq!(body["error"]["code"], status.as_u16(), "{what}: {body}");
        assert!(!message(&body).is_empty(), "{what}: {body}");
    }
}

/// Trino 483 writes only through single-table commits (live check). This is
/// the Iceberg REST shape with an `identifier` equal to the path (clients other
/// than Trino send one; Trino sends none, see the captured body below), with
/// `assert-table-uuid` and `assert-current-schema-id`, then the updates. Such a
/// commit passes the guard for a role with write on the table: single- and
/// multi-level namespaces, a name with a space (path `%20` or `+`), and a grant
/// spelled in another case.
#[tokio::test]
async fn a_trino_shaped_table_commit_passes_the_guard() {
    let env = env_or_skip!();
    env.create_table("sales", "orders").await;
    env.create_table("corp%1Ffin", "ledger").await;
    env.create_table("sales", "my table").await;
    let token = env
        .token_for(
            "external",
            &[
                grant("table", Some(&["sales"]), Some("Orders"), "write"),
                grant("table", Some(&["corp", "fin"]), Some("ledger"), "write"),
                grant("table", Some(&["sales"]), Some("my table"), "write"),
            ],
        )
        .await;
    let cases: [(&str, &str, &[&str], &str); 4] = [
        ("sales", "orders", &["sales"], "orders"),
        ("corp%1Ffin", "ledger", &["corp", "fin"], "ledger"),
        ("sales", "my%20table", &["sales"], "my table"),
        ("sales", "my+table", &["sales"], "my table"),
    ];
    for (i, (path_ns, path_name, ns, name)) in cases.into_iter().enumerate() {
        let (_, loaded) = env
            .send(
                &env.raw,
                None,
                "GET",
                &format!("namespaces/{path_ns}/tables/{path_name}"),
                Value::Null,
            )
            .await;
        let uuid = loaded["metadata"]["table-uuid"].clone();
        let schema_id = loaded["metadata"]["current-schema-id"].clone();
        let body = json!({
            "identifier": {"namespace": ns, "name": name},
            "requirements": [
                {"type": "assert-table-uuid", "uuid": uuid},
                {"type": "assert-current-schema-id", "current-schema-id": schema_id},
            ],
            "updates": [
                {"action": "set-properties", "updates": {"egret.probe": format!("trino{i}")}},
                {"action": "add-schema", "schema": {"type": "struct", "schema-id": 1, "fields": [
                    {"id": 1, "name": "id", "type": "string", "required": false},
                    {"id": 2, "name": format!("c{i}"), "type": "long", "required": false},
                ]}, "last-column-id": 2},
                {"action": "set-current-schema", "schema-id": -1},
            ],
        });
        let started = std::time::Instant::now();
        let (status, resp) = env
            .commit_one(&token, "tables", path_ns, path_name, body)
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{path_ns}/{path_name}: {resp} (after {:?})",
            started.elapsed()
        );
        assert_eq!(
            env.probe(path_ns, path_name).await.as_deref(),
            Some(format!("trino{i}").as_str()),
            "{path_ns}/{path_name}"
        );
    }
}

/// A commit body captured from Trino 483 in the second live check (an `ALTER
/// TABLE … ADD COLUMN phone`), verbatim except the table uuid. Trino sends no
/// `identifier`, so the guard authorizes the path's table.
#[tokio::test]
async fn a_captured_trino_commit_passes_the_guard() {
    let env = env_or_skip!();
    let schema = json!({"type": "struct", "schema-id": 0, "fields": [
        {"id": 1, "name": "id", "required": false, "type": "int"},
        {"id": 2, "name": "email", "required": false, "type": "string"},
        {"id": 3, "name": "region", "required": false, "type": "string"},
    ]});
    let (status, created) = env
        .send(
            &env.raw,
            None,
            "POST",
            "namespaces/sales/tables",
            json!({"name": "orders", "schema": schema}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let uuid = created["metadata"]["table-uuid"].clone();
    let body = json!({
        "requirements": [
            {"type": "assert-table-uuid", "uuid": uuid},
            {"type": "assert-last-assigned-field-id", "last-assigned-field-id": 3},
            {"type": "assert-current-schema-id", "current-schema-id": 0},
        ],
        "updates": [
            {"action": "add-schema", "schema": {"type": "struct", "schema-id": 1, "fields": [
                {"id": 1, "name": "id", "required": false, "type": "int"},
                {"id": 2, "name": "email", "required": false, "type": "string"},
                {"id": 3, "name": "region", "required": false, "type": "string"},
                {"id": 4, "name": "phone", "required": false, "type": "string"},
            ]}, "last-column-id": 4},
            {"action": "set-current-schema", "schema-id": -1},
        ],
    });
    let writer = env
        .token_for(
            "external",
            &[grant("table", Some(&["sales"]), Some("orders"), "write")],
        )
        .await;
    let reader = env
        .token_for(
            "external",
            &[grant("table", Some(&["sales"]), Some("orders"), "read")],
        )
        .await;
    let (status, resp) = env
        .commit_one(&reader, "tables", "sales", "orders", body.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a read grant: {resp}");
    let (status, resp) = env
        .commit_one(&writer, "tables", "sales", "orders", body)
        .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["metadata"]["current-schema-id"], 1, "{resp}");
}
