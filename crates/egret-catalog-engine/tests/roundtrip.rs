//! End-to-end behavioral proof of the embedded-lakekeeper strategy (Task 4 /
//! CE-1 headline): drive the **real** lakekeeper Iceberg REST router in-process
//! (no socket) and round-trip a catalog op through it — config → create
//! namespace → create table → load table — then assert the row landed in
//! lakekeeper's own schema.
//!
//! DB + MinIO gated: self-skips when `DATABASE_URL` is unset. The warehouse
//! creation step (`ensure_spike_warehouse`) does a LIVE S3 `validate_access`
//! against the dev MinIO, so MinIO must be reachable at `localhost:9000` with a
//! writable `warehouse` bucket (shipped by the egret dev stack).
//!
//! ## KEY FINDING — injecting `RequestMetadata` (CE-3: token→principal bridge)
//!
//! `new_v1_full_router` (the bare router `mount.rs` builds) carries NO authn
//! middleware. Every catalog handler extracts `Extension<RequestMetadata>` from
//! the request extensions — in the real server that extension is inserted by
//! lakekeeper's authn layer (`request_metadata.rs::create_request_metadata_*`,
//! see also `api/router.rs`'s `auth_middleware_fn`). Without it, every handler
//! 500s on a missing extension.
//!
//! We mirror that layer with a single `axum::middleware::from_fn` that inserts a
//! `RequestMetadata` carrying a full-scope `egret` claim (CE-4): a token-shaped
//! `Actor::Principal` with `{ perms: [read, write], ns: [] }`. The control-plane
//! actor (`new_lakekeeper_internal`) bypasses control-plane authz, but CE-4's
//! [`egret_catalog_engine::EgretAuthorizer`] now gates DATA actions (namespace /
//! table) off the `egret` claim — an unscoped internal actor fails closed there.
//! An empty `ns` scope means "all namespaces", so this full-scope claim lets the
//! round-trip proceed while still exercising the real authorizer's claim path
//! (not its system bypass). Its `project_id` is `None`, which
//! `require_project_id(None)` resolves to the OSS `DEFAULT_PROJECT_ID` (nil
//! project) that `bootstrap` seeded — so `GET /config?warehouse=spike` resolves
//! the warehouse in the default project.
//!
//! This mirrors the prod token→principal bridge in `front_auth::front_auth`.
//!
//! ## KEY FINDING — the route shape lakekeeper uses (CE-2: provisioning)
//!
//! The bare `new_v1_full_router` mounts routes at the TOP level (the real
//! server nests them under `/catalog/v1`, see `api/router.rs`):
//!   * `GET  /config?warehouse=<name>`
//!   * `POST /{prefix}/namespaces`
//!   * `POST /{prefix}/namespaces/{namespace}/tables`
//!   * `GET  /{prefix}/namespaces/{namespace}/tables/{table}`
//!
//! where `{prefix}` is the warehouse's **UUID** (`Prefix: TryFrom -> WarehouseId`,
//! and the default `prefix_template = "{warehouse_id}"`). The warehouse *name*
//! ("spike") is used ONLY in the `?warehouse=` config query; the config
//! response's `defaults.prefix` hands back the UUID to use as `{prefix}` for all
//! subsequent calls. We follow that exact discovery flow here.

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use egret_catalog_engine::{
    build_state, catalog_engine_router, ensure_spike_warehouse, run_lakekeeper_migrations,
};
use http_body_util::BodyExt;
use lakekeeper::api::RequestMetadata;
use lakekeeper::limes::Authentication;
use lakekeeper::service::authn::{Actor, UserId};
use tower::ServiceExt; // for `oneshot`
use uuid::Uuid;

/// Mirror lakekeeper's authn layer for the headless router: insert a
/// `RequestMetadata` so the catalog handlers' `Extension<_>` extractor succeeds.
///
/// CE-4: the engine now runs `EgretAuthorizer`, which gates DATA actions off the
/// `egret` authentication claim (fail-closed without it). So we inject a
/// token-shaped principal carrying a FULL-scope claim (`perms: [read, write]`,
/// empty `ns` = all namespaces) — the same shape the prod front-auth bridge
/// builds — so the round-trip proceeds through the real authorizer's claim path.
async fn inject_request_metadata(mut request: Request, next: Next) -> Response {
    let uid = UserId::try_from(
        format!(
            "oidc~{}",
            Uuid::new_v5(&Uuid::NAMESPACE_OID, b"roundtrip-token")
        )
        .as_str(),
    )
    .expect("valid principal id");
    let authentication = Authentication::builder()
        .token_header(None)
        .claims(serde_json::json!({
            "egret": { "perms": ["catalog:read", "catalog:write"], "ns": [] }
        }))
        .subject(uid.clone().into())
        .name(Some("Roundtrip Token".to_string()))
        .email(None)
        .principal_type(None)
        .build();
    let mut md = RequestMetadata::new_lakekeeper_internal(Uuid::now_v7());
    md.set_authentication(Actor::Principal(uid), authentication);
    request.extensions_mut().insert(md);
    next.run(request).await
}

/// Collect a response body into JSON, surfacing the status + raw body on
/// failure so assertions report what lakekeeper actually returned.
async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

#[tokio::test]
async fn iceberg_roundtrip_through_lakekeeper() {
    // DB + MinIO gate.
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("skip iceberg_roundtrip_through_lakekeeper: DATABASE_URL not set");
        return;
    };

    // 1. Migrate lakekeeper's catalog into its isolated `lakekeeper` schema,
    //    build the real router state, and provision the `spike` warehouse
    //    (bootstrap seeds the default project; create_warehouse does a LIVE
    //    MinIO validate_access against the `warehouse` bucket).
    run_lakekeeper_migrations(&dsn)
        .await
        .expect("run lakekeeper migrations");
    // `system_user` only matters for the authorizer's system-principal bypass,
    // which this test does NOT use (it drives the engine via a full-scope egret
    // claim, see `inject_request_metadata`). Any value is fine.
    let state = build_state(&dsn, "egret-system")
        .await
        .expect("build lakekeeper router state");
    ensure_spike_warehouse(state.clone())
        .await
        .expect("provision spike warehouse (needs MinIO reachable + writable warehouse bucket)");

    // 2. Build the mounted catalog router + the RequestMetadata-injecting layer.
    let app = catalog_engine_router(state.clone())
        .layer(axum::middleware::from_fn(inject_request_metadata));

    // 3a. GET /config?warehouse=spike -> 200, and discover the {prefix}
    //     (warehouse UUID) to use for all subsequent paths.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/config?warehouse=spike")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, config) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "GET /config; body={config}");
    let prefix = config["defaults"]["prefix"]
        .as_str()
        .unwrap_or_else(|| panic!("config.defaults.prefix missing; body={config}"))
        .to_string();
    // Sanity: the prefix is a UUID (the warehouse id, per prefix_template).
    Uuid::parse_str(&prefix).expect("config prefix should be the warehouse UUID");

    // Unique namespace/table per run so the test is re-runnable against a
    // persistent catalog (the `spike` warehouse + bootstrap are idempotent, but
    // namespaces/tables are not — a fixed name would 409 on the second run).
    let ns_name = format!("demo_{}", Uuid::now_v7().simple());
    let table_name = "t";

    // 3b. POST /{prefix}/namespaces — create namespace `demo_<uuid>`.
    let body = serde_json::json!({ "namespace": [ns_name], "properties": {} });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix}/namespaces"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, ns) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create namespace; body={ns}");
    assert_eq!(ns["namespace"][0], ns_name, "namespace echoed; body={ns}");

    // 3c. POST /{prefix}/namespaces/demo/tables — create table `t` with a
    //     minimal single-`long`-column Iceberg schema. No explicit location:
    //     it derives from the warehouse's storage profile (the spike MinIO
    //     prefix). Standard Iceberg REST v2 create-table body shape.
    let create_table = serde_json::json!({
        "name": table_name,
        "schema": {
            "type": "struct",
            "schema-id": 0,
            "fields": [
                { "id": 1, "name": "id", "required": true, "type": "long" }
            ]
        }
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{prefix}/namespaces/{ns_name}/tables"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_table).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, created) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create table; body={created}");
    assert!(
        created["metadata-location"].is_string(),
        "created table has a metadata-location; body={created}"
    );
    let created_uuid = created["metadata"]["table-uuid"]
        .as_str()
        .unwrap_or_else(|| panic!("created table has a table-uuid; body={created}"))
        .to_string();

    // 3d. GET /{prefix}/namespaces/demo/tables/t — load it back and assert the
    //     schema round-tripped (same table-uuid, the `id: long` column survives).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/{prefix}/namespaces/{ns_name}/tables/{table_name}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, loaded) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "load table; body={loaded}");
    assert_eq!(
        loaded["metadata"]["table-uuid"], created_uuid,
        "loaded table-uuid matches the created one; body={loaded}"
    );
    // The schema we sent must survive the round-trip through lakekeeper's
    // metadata machinery.
    let schemas = &loaded["metadata"]["schemas"];
    let field = &schemas[0]["fields"][0];
    assert_eq!(
        field["name"], "id",
        "column name round-trips; body={loaded}"
    );
    assert_eq!(
        field["type"], "long",
        "column type round-trips; body={loaded}"
    );

    // 4. Assert the table actually persisted in lakekeeper's schema (not just
    //    echoed through the API). Query the isolated `lakekeeper."table"`.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(|conn, _| {
            Box::pin(async move {
                use sqlx::Executor;
                conn.execute("SET search_path = lakekeeper, public").await?;
                Ok(())
            })
        })
        .connect(&dsn)
        .await
        .expect("connect for persistence assertion");
    let (count,): (i64,) = sqlx::query_as(r#"SELECT count(*) FROM "tabular""#)
        .fetch_one(&pool)
        .await
        .expect("count rows in lakekeeper.tabular");
    assert!(
        count > 0,
        "expected at least one tabular row persisted in lakekeeper schema, got {count}"
    );
    pool.close().await;
}
