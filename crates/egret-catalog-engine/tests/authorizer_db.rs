//! `EgretAuthorizer` through lakekeeper's real router, with the caller's
//! request metadata set directly (no guard in front): the questions the guard
//! never lets reach it from a request (vended-credentials spec §4.4, §4.5).
//!
//! `GET …/tables/{t}/credentials` answers 403 when lakekeeper's authorizer
//! allows neither `ReadData` nor `WriteData`, so it shows the data decision on
//! an in-memory warehouse, with no object store.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use axum::body::Body;
use axum::http::StatusCode;
use egret_catalog_engine::{
    build_state, catalog_client_metadata, catalog_engine_router, create_memory_warehouse,
    ensure_bootstrapped, front_catalog_router, run_lakekeeper_migrations, CatalogClientIdentity,
    CatalogStore, GrantInput,
};
use lakekeeper::api::RequestMetadata;
use lakekeeper::limes::Authentication;
use lakekeeper::service::{Actor, UserId};
use serde_json::json;
use tower::ServiceExt as _;

const SYSTEM_USER: &str = "egret-system";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    )
}

static LAKEKEEPER: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

struct Env {
    /// Lakekeeper's router with no layer in front: the test sets the metadata.
    bare: axum::Router,
    /// The same with the internal actor, for setup.
    raw: axum::Router,
    store: CatalogStore,
    db: egret_metastore::Db,
    /// The lakekeeper warehouse the table lives in.
    prefix: String,
    /// Another lakekeeper warehouse.
    other_prefix: String,
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
                .expect("lakekeeper migrations");
            let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
            ensure_bootstrapped(state, SYSTEM_USER)
                .await
                .expect("bootstrap");
        })
        .await;
    let (state, _) = build_state(&dsn, SYSTEM_USER).await.expect("state");
    let prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("azwh"))
        .await
        .expect("warehouse");
    let other_prefix = create_memory_warehouse(state.clone(), SYSTEM_USER, &unique("azwh"))
        .await
        .expect("other warehouse");
    let env = Env {
        bare: catalog_engine_router(state.clone()),
        raw: front_catalog_router(catalog_engine_router(state), None, None),
        store: CatalogStore::new(db.clone()),
        db,
        prefix,
        other_prefix,
    };
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
            .uri(format!("/{}/{path}", env.prefix))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = env.raw.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "setup {path}");
    }
    Some(env)
}

impl Env {
    /// `GET /credentials` of `sales.orders` as `metadata`.
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

    /// An egret warehouse bound to the lakekeeper warehouse `lk`, and a role
    /// in it with `catalog` `read`. Returns (warehouse name, role name).
    async fn egret_role_on(&self, lk: &str) -> (String, String) {
        let pool = &self.db.pool;
        let ws: uuid::Uuid =
            sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'AZ') RETURNING id")
                .bind(unique("az-ws"))
                .fetch_one(pool)
                .await
                .expect("workspace");
        let sc: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
             VALUES ($1,'us-east-1','b','AK') RETURNING id",
        )
        .bind(unique("az-sc"))
        .fetch_one(pool)
        .await
        .expect("storage connection");
        let warehouse = unique("az_wh");
        let warehouse_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
             lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4) RETURNING id",
        )
        .bind(&warehouse)
        .bind(ws)
        .bind(sc)
        .bind(lk)
        .fetch_one(pool)
        .await
        .expect("warehouse");
        let role_name = unique("az-role");
        let role = self
            .store
            .create_role(warehouse_id, &role_name, "external", None)
            .await
            .expect("role");
        self.store
            .add_grant(
                role.id,
                &GrantInput {
                    securable_type: "catalog".into(),
                    namespace: None,
                    table_name: None,
                    privilege: "read".into(),
                },
            )
            .await
            .expect("grant");
        (warehouse, role_name)
    }
}

fn client(warehouse: String, role: String) -> RequestMetadata {
    catalog_client_metadata(&CatalogClientIdentity {
        warehouse,
        role,
        subject: unique("az-client"),
    })
}

/// The control: a catalog client whose warehouse is the one lakekeeper
/// resolved, holding a read grant, may read the table's data.
#[tokio::test]
async fn a_client_of_the_warehouse_may_read() {
    let Some(env) = env().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let (warehouse, role) = env.egret_role_on(&env.prefix).await;

    assert_eq!(
        env.credentials(client(warehouse, role)).await,
        StatusCode::OK
    );
}

/// C5: a client whose token names a warehouse bound to another lakekeeper
/// warehouse gets no data access in this one, whatever its grants. The guard
/// already refuses such a request; this is the authorizer's own check.
#[tokio::test]
async fn a_client_of_another_warehouse_gets_no_data_access() {
    let Some(env) = env().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let (warehouse, role) = env.egret_role_on(&env.other_prefix).await;

    assert_eq!(
        env.credentials(client(warehouse, role)).await,
        StatusCode::FORBIDDEN
    );
}

/// C4: a principal that is neither a catalog client nor egret's own gets no
/// data access (fail closed).
#[tokio::test]
async fn an_unknown_principal_gets_no_data_access() {
    let Some(env) = env().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let user_id = UserId::try_from("oidc~stranger").expect("a user id");
    let authentication = Authentication::builder()
        .token_header(None)
        .claims(json!({}))
        .subject(user_id.clone().into())
        .name(None)
        .email(None)
        .principal_type(None)
        .build();
    let mut stranger = RequestMetadata::new_lakekeeper_internal(uuid::Uuid::now_v7());
    stranger.set_authentication(Actor::Principal(user_id), authentication);

    assert_eq!(env.credentials(stranger).await, StatusCode::FORBIDDEN);
}
