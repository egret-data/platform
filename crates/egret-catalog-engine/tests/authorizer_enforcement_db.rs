//! SP5c Task 3: with a catalog's iceberg-rest switch on, `EgretAuthorizer`
//! answers `ReadData` false for a table carrying a mask, so lakekeeper vends no
//! storage credential for it (`GET …/credentials` is 403 when neither read nor
//! write data is allowed). A restricted table is denied for write asks too.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use axum::body::Body;
use axum::http::StatusCode;
use egret_catalog_engine::{
    build_state, catalog_client_metadata, catalog_engine_router, create_memory_warehouse,
    ensure_bootstrapped, front_catalog_router, run_lakekeeper_migrations, CatalogClientIdentity,
};
use egret_perm_compile::catalog_rbac_migrate::fold_catalog;
use lakekeeper::api::RequestMetadata;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt as _;
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::new_v4().simple().to_string()[..12])
}

static LAKEKEEPER: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

struct Env {
    bare: axum::Router,
    pool: PgPool,
    prefix: String,
    warehouse: String,
    warehouse_id: Uuid,
    catalog_id: Uuid,
    ws: Uuid,
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = egret_metastore::Db::connect(&dsn, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    LAKEKEEPER
        .get_or_init(|| async {
            run_lakekeeper_migrations(&dsn)
                .await
                .expect("lk migrations");
            let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
            ensure_bootstrapped(state, SYSTEM_USER)
                .await
                .expect("bootstrap");
        })
        .await;
    let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("enwh"))
        .await
        .expect("warehouse");
    let raw = front_catalog_router(catalog_engine_router(state.clone()), None, None);
    let pool = db.pool.clone();
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'EN') RETURNING id")
            .bind(unique("en-ws"))
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(unique("en-sc"))
    .fetch_one(&pool)
    .await
    .expect("storage connection");
    let warehouse = unique("en_wh");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
         lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(&warehouse)
    .bind(ws)
    .bind(sc)
    .bind(&prefix)
    .fetch_one(&pool)
    .await
    .expect("warehouse");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id, storage_connection_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
            jsonb_build_object('warehouse_id', $2::text), \
            'storage_connection_id', $3::text), $2, $3) RETURNING id",
    )
    .bind(unique("encat"))
    .bind(warehouse_id)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .expect("registry catalog");
    for (path, body) in [
        ("namespaces", json!({"namespace": ["sales"]})),
        (
            "namespaces/sales/tables",
            json!({"name": "orders", "schema": {"type": "struct", "schema-id": 0, "fields": [
                {"id": 1, "name": "id", "type": "string", "required": false}]}}),
        ),
    ] {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/{prefix}/{path}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = raw.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "setup {path}");
    }
    Some(Env {
        bare: catalog_engine_router(state),
        pool,
        prefix,
        warehouse,
        warehouse_id,
        catalog_id,
        ws,
    })
}

impl Env {
    /// A catalog client (role + service account) holding `privilege` on
    /// `catalog`, folded into `perm.grants`. Returns (metadata, username).
    async fn client(&self, privilege: &str) -> (RequestMetadata, String) {
        let pool = &self.pool;
        let role_name = unique("en-role");
        let role: Uuid = sqlx::query_scalar(
            "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) \
             VALUES ($1,$2,'external') RETURNING id",
        )
        .bind(self.warehouse_id)
        .bind(&role_name)
        .fetch_one(pool)
        .await
        .expect("role");
        sqlx::query(
            "INSERT INTO catalog.catalog_grants \
               (catalog_role_id, securable_type, privilege) VALUES ($1,'catalog',$2)",
        )
        .bind(role)
        .bind(privilege)
        .execute(pool)
        .await
        .expect("grant");
        let username = unique("enuser");
        let sa: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, username) \
             VALUES ('local', $1, $1, $2) RETURNING id",
        )
        .bind(format!("{username}@service.invalid"))
        .bind(&username)
        .fetch_one(pool)
        .await
        .expect("user");
        sqlx::query(
            "INSERT INTO service_accounts (user_id, scope, workspace_id, name) \
             VALUES ($1,'workspace',$2,$3)",
        )
        .bind(sa)
        .bind(self.ws)
        .bind(format!("n-{sa}"))
        .execute(pool)
        .await
        .expect("sa");
        let client_id = unique("en-cid");
        sqlx::query(
            "INSERT INTO catalog.catalog_clients \
               (name, consumer_type, workspace_id, warehouse_id, catalog_role_id, client_id, \
                client_secret_hash, service_account_id) \
             VALUES ($1,'external',$2,$3,$4,$5,'h',$6)",
        )
        .bind(unique("c-"))
        .bind(self.ws)
        .bind(self.warehouse_id)
        .bind(role)
        .bind(&client_id)
        .bind(sa)
        .execute(pool)
        .await
        .expect("client");
        let mut c = pool.acquire().await.unwrap();
        fold_catalog(&mut c, self.catalog_id, "sp5c-test")
            .await
            .expect("fold");
        (
            catalog_client_metadata(&CatalogClientIdentity {
                warehouse: self.warehouse.clone(),
                role: role_name,
                subject: client_id,
            }),
            username,
        )
    }

    async fn set_unified(&self) {
        sqlx::query(
            "INSERT INTO perm.catalog_grant_switch (catalog_id, source, switched_at, switched_by) \
             VALUES ($1,'unified',now(),'sp5c-test') \
             ON CONFLICT (catalog_id) DO UPDATE SET source = 'unified'",
        )
        .bind(self.catalog_id)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn set_enforce(&self, on: bool) {
        sqlx::query(
            "INSERT INTO perm.catalog_rule_enforcement_switch \
               (catalog_id, enforce, switched_at, switched_by) VALUES ($1,$2,now(),'sp5c-test') \
             ON CONFLICT (catalog_id) DO UPDATE SET enforce = EXCLUDED.enforce",
        )
        .bind(self.catalog_id)
        .bind(on)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    /// A mask on `sales.orders.id`, with `exempt` (usernames) cleared of it.
    async fn mask_orders(&self, exempt: &[&str]) {
        let rule: Uuid = sqlx::query_scalar(
            "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, \
                                     schema_name, table_name, column_name) \
             VALUES ($1,'mask'::perm.rule_kind,'MASK_NULL','path'::perm.selector_kind, \
                     $2,'sales','orders','id') RETURNING id",
        )
        .bind(unique("sp5c-"))
        .bind(self.catalog_id)
        .fetch_one(&self.pool)
        .await
        .expect("rule");
        for user in exempt {
            sqlx::query(
                "INSERT INTO perm.rule_exemptions (rule_id, kind, ref, mode) \
                 VALUES ($1,'user',$2,'include')",
            )
            .bind(rule)
            .bind(*user)
            .execute(&self.pool)
            .await
            .expect("exemption");
        }
    }

    async fn credentials(&self, metadata: RequestMetadata) -> StatusCode {
        let mut req = axum::http::Request::builder()
            .method("GET")
            .uri(format!(
                "/{}/namespaces/sales/tables/orders/credentials",
                self.prefix
            ))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(metadata);
        self.bare.clone().oneshot(req).await.unwrap().status()
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

#[tokio::test]
async fn a_read_of_a_masked_table_vends_no_credential_when_enforcing() {
    let env = env_or_skip!();
    env.set_unified().await;
    let (reader, _) = env.client("read").await;
    assert_eq!(
        env.credentials(reader.clone()).await,
        StatusCode::OK,
        "control"
    );
    env.mask_orders(&[]).await;

    // Opt-in: no enforcement row, then enforce=off, still reads.
    assert_eq!(env.credentials(reader.clone()).await, StatusCode::OK);
    env.set_enforce(false).await;
    assert_eq!(env.credentials(reader.clone()).await, StatusCode::OK);

    env.set_enforce(true).await;
    assert_eq!(env.credentials(reader).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_exempt_principal_still_reads_a_masked_table() {
    let env = env_or_skip!();
    env.set_unified().await;
    env.set_enforce(true).await;
    let (reader, username) = env.client("read").await;
    env.mask_orders(&[&username]).await;

    assert_eq!(env.credentials(reader).await, StatusCode::OK);
}

#[tokio::test]
async fn a_write_ask_on_a_restricted_table_vends_no_credential() {
    let env = env_or_skip!();
    env.set_unified().await;
    env.mask_orders(&[]).await;
    let (writer, _) = env.client("write").await;

    // Opt-in: not enforcing, the write grant vends a credential.
    assert_eq!(env.credentials(writer.clone()).await, StatusCode::OK);
    env.set_enforce(false).await;
    assert_eq!(env.credentials(writer.clone()).await, StatusCode::OK);

    // Enforcing: a write credential could read the raw files, so none is vended.
    env.set_enforce(true).await;
    assert_eq!(env.credentials(writer).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_exempt_writer_still_gets_a_credential() {
    let env = env_or_skip!();
    env.set_unified().await;
    env.set_enforce(true).await;
    let (writer, username) = env.client("write").await;
    env.mask_orders(&[&username]).await;

    assert_eq!(env.credentials(writer).await, StatusCode::OK);
}

#[tokio::test]
async fn an_unruled_table_is_unaffected_when_enforcing() {
    let env = env_or_skip!();
    env.set_unified().await;
    env.set_enforce(true).await;
    // The only rule is on another table.
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, \
                                 schema_name, table_name, column_name) \
         VALUES ($1,'mask'::perm.rule_kind,'MASK_NULL','path'::perm.selector_kind, \
                 $2,'sales','elsewhere','id')",
    )
    .bind(unique("sp5c-"))
    .bind(env.catalog_id)
    .execute(&env.pool)
    .await
    .unwrap();
    let (writer, _) = env.client("write").await;
    assert_eq!(env.credentials(writer).await, StatusCode::OK);
}

/// FAIL CLOSED: when enforcement cannot be established (here the connection is
/// in an aborted transaction, so every query errs), a read is DENIED — an `Err`
/// is never read as "nothing restricts". (The snapshot-build `Err` arm maps to
/// the same `DenyAll`; `build_catalog_enforcement_snapshot`'s own refusal is
/// pinned in egret-perm-compile.)
#[tokio::test]
async fn an_unenforceable_catalog_denies_accesss() {
    use egret_catalog_engine::unified_decide::{CatalogEnforcementSnapshotCache, ReadEnforcement};
    let env = env_or_skip!();
    env.set_unified().await;
    env.set_enforce(true).await;
    let cache = CatalogEnforcementSnapshotCache::new();
    let ns = ["sales".to_string()];

    // Control: a healthy connection and no rule restricts nothing.
    let mut c = env.pool.acquire().await.unwrap();
    let healthy = ReadEnforcement::load(&cache, &mut c, env.catalog_id).await;
    assert!(!healthy.denies_access("anyone", "c", &ns, "orders"));
    drop(c);

    let mut tx = env.pool.begin().await.unwrap();
    assert!(sqlx::query("SELECT 1/0").execute(&mut *tx).await.is_err());
    let broken = ReadEnforcement::load(&cache, &mut tx, env.catalog_id).await;
    assert!(
        broken.denies_access("anyone", "c", &ns, "orders"),
        "an Err must deny"
    );
    assert!(broken.denies_access("anyone", "c", &ns, "unrelated"));
}
