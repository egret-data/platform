//! Session-authenticated catalog **table-detail enrichment** tests (catalog
//! sub-epic 3). DB + MinIO gated; self-skips without `DATABASE_URL`.
//!
//! Proves the widened `table_detail` DTO projects the iceberg `TableMetadata`
//! the load already returns — partition spec, sort order, snapshot history,
//! format version, last-updated — with NO query execution:
//!   - a PARTITIONED table reports `format_version` ∈ {1,2,3}, its one
//!     `partition_field` with the source column NAME resolved + transform, the
//!     matching `columns`, an empty `sort_fields`, and (fresh, uncommitted)
//!     empty `snapshots`,
//!   - an UNPARTITIONED table reports `partition_fields == []`.
//!
//! Seeding mirrors `browse_api.rs`: provision a per-workspace warehouse, drive
//! the bare engine router (injected in-process principal) to create the
//! namespace + tables, then read detail through the session-authed
//! `browse_router` as a `ws_viewer` member.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test table_detail_api -- --test-threads=1

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    browse_router, build_state, ensure_bootstrapped, provision_workspace_warehouse,
    run_lakekeeper_migrations, BrowseState,
};
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_storage::StorageConfig;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use lakekeeper::api::RequestMetadata;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

fn dev_storage_config() -> StorageConfig {
    StorageConfig {
        endpoint: Some("http://localhost:9000".to_string()),
        region: "local-01".to_string(),
        bucket: "warehouse".to_string(),
        access_key: "egret".to_string(),
        secret_key: "egretsecret".to_string(),
        path_style: true,
    }
}

struct Fixture {
    app: axum::Router,
    seed_app: axum::Router,
    sessions: SessionService,
    users: UserRepo,
    members: MemberRepo,
    workspace_id: WorkspaceId,
    owner: UserId,
    slug: String,
    /// The egret warehouse-ROW id (`{wid}` path segment of the browse routes).
    wid: Uuid,
    /// The lakekeeper warehouse UUID used as the seed `{prefix}`.
    prefix: String,
    ns_name: String,
}

/// Mirror lakekeeper's authn layer for the bare engine router (used only to
/// seed data): inject an in-process `RequestMetadata` so the catalog handlers'
/// `Extension<_>` extractor succeeds.
async fn inject_request_metadata(mut request: Request, next: Next) -> Response {
    request
        .extensions_mut()
        .insert(RequestMetadata::new_lakekeeper_internal(Uuid::now_v7()));
    next.run(request).await
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

async fn setup() -> Option<Fixture> {
    let dsn = std::env::var("DATABASE_URL").ok()?;

    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations");
    let (engine_state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap");

    let store = CatalogStore::new(db.clone());
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let suffix = Uuid::now_v7().simple().to_string();
    let owner = users
        .insert_local(&format!("tdowner-{suffix}@egret.test"), "TD Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("tdws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Table-Detail Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    let warehouse_id = provision_workspace_warehouse(
        engine_state.clone(),
        &store,
        workspace_id.0,
        &slug,
        &dev_storage_config(),
        SYSTEM_USER,
    )
    .await
    .expect("provision warehouse (needs MinIO reachable + writable `warehouse` bucket)");

    let wid = store
        .get_warehouse_by_name(workspace_id.0, "default")
        .await
        .expect("default warehouse row")
        .id;

    let seed_app = egret_catalog_engine::catalog_engine_router(engine_state.clone())
        .layer(axum::middleware::from_fn(inject_request_metadata));
    let prefix = warehouse_id.to_string();
    let ns_name = format!("demo_{}", Uuid::now_v7().simple());

    // POST /{prefix}/namespaces
    let body = serde_json::json!({ "namespace": [ns_name], "properties": {} });
    let resp = seed_app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!("/{prefix}/namespaces"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, ns) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "seed namespace; body={ns}");

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secret_store = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        workspace_secrets: egret_secrets::WorkspaceSecretStore::new(db.clone(), secret_store),
        system_user: SYSTEM_USER.into(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
    });

    Some(Fixture {
        app,
        seed_app,
        sessions,
        users,
        members: MemberRepo::new(db.clone()),
        workspace_id,
        owner,
        slug,
        wid,
        prefix,
        ns_name,
    })
}

/// Add a new user as a workspace member with `role`, returning a session token.
async fn add_member(f: &Fixture, email_tag: &str, role: &str) -> String {
    let email = format!("{email_tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.members
        .add(f.workspace_id, "user", uid.0, role, f.owner)
        .await
        .expect("seed member");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Create a table under the seeded namespace via the bare engine router,
/// tolerating ONE transient MinIO write flake (retry once).
async fn create_table(f: &Fixture, body: &serde_json::Value) -> (StatusCode, serde_json::Value) {
    let uri = format!("/{}/namespaces/{}/tables", f.prefix, f.ns_name);
    let send = || async {
        let resp = f
            .seed_app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri(&uri)
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        into_json(resp).await
    };
    let (s, j) = send().await;
    if s == StatusCode::OK {
        (s, j)
    } else {
        eprintln!("create table first attempt non-OK ({s}); retrying once. body={j}");
        send().await
    }
}

/// GET the session-authed table detail as `token`.
async fn detail(f: &Fixture, token: &str, table: &str) -> (StatusCode, serde_json::Value) {
    let uri = format!(
        "/ws/{}/catalog/warehouses/{}/namespaces/{}/tables/{}",
        f.slug, f.wid, f.ns_name, table
    );
    let resp = f
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(&uri)
                .header("Cookie", format!("egret_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    into_json(resp).await
}

#[tokio::test]
async fn partitioned_table_detail_reports_spec_format_and_empty_history() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    // A partitioned table: identity-partition on the `country` column (id 2).
    let table_name = format!("partitioned_{}", Uuid::now_v7().simple());
    let create = serde_json::json!({
        "name": table_name,
        "schema": {
            "type": "struct",
            "schema-id": 0,
            "fields": [
                { "id": 1, "name": "id", "required": true, "type": "long" },
                { "id": 2, "name": "country", "required": false, "type": "string" }
            ]
        },
        "partition-spec": {
            "spec-id": 0,
            "fields": [
                { "source-id": 2, "field-id": 1000, "name": "country", "transform": "identity" }
            ]
        }
    });
    let (status, created) = create_table(&f, &create).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create partitioned table; body={created}"
    );

    let (status, json) = detail(&f, &token, &table_name).await;
    assert_eq!(status, StatusCode::OK, "partitioned detail; body={json}");

    // Format version is a small integer (V1/V2/V3).
    let fv = json["format_version"].as_u64().expect("format_version u64");
    assert!(
        (1..=3).contains(&fv),
        "format_version in 1..=3; body={json}"
    );

    // last_updated_ms present and positive.
    assert!(
        json["last_updated_ms"].as_i64().unwrap_or(0) > 0,
        "last_updated_ms positive; body={json}"
    );

    // Columns mirror the schema.
    assert_eq!(json["columns"][0]["name"], "id");
    assert_eq!(json["columns"][0]["type"], "long");
    assert_eq!(json["columns"][1]["name"], "country");

    // Partition fields: exactly one, with the source COLUMN NAME resolved.
    let pfs = json["partition_fields"]
        .as_array()
        .expect("partition_fields array");
    assert_eq!(pfs.len(), 1, "one partition field; body={json}");
    assert_eq!(
        pfs[0]["name"], "country",
        "partition field name; body={json}"
    );
    assert_eq!(
        pfs[0]["source_column"], "country",
        "source_column resolved to the schema column name; body={json}"
    );
    assert_eq!(pfs[0]["transform"], "identity", "transform; body={json}");

    // Fresh table: unsorted + no commits.
    assert_eq!(
        json["sort_fields"],
        serde_json::json!([]),
        "no sort fields; body={json}"
    );
    assert_eq!(
        json["snapshots"],
        serde_json::json!([]),
        "no snapshots on a fresh table; body={json}"
    );
}

#[tokio::test]
async fn unpartitioned_table_detail_has_empty_partition_fields() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    let table_name = format!("unpartitioned_{}", Uuid::now_v7().simple());
    let create = serde_json::json!({
        "name": table_name,
        "schema": {
            "type": "struct",
            "schema-id": 0,
            "fields": [ { "id": 1, "name": "id", "required": true, "type": "long" } ]
        }
    });
    let (status, created) = create_table(&f, &create).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create unpartitioned table; body={created}"
    );

    let (status, json) = detail(&f, &token, &table_name).await;
    assert_eq!(status, StatusCode::OK, "unpartitioned detail; body={json}");

    assert_eq!(
        json["partition_fields"],
        serde_json::json!([]),
        "unpartitioned → empty partition_fields; body={json}"
    );
    assert_eq!(json["sort_fields"], serde_json::json!([]));
    assert_eq!(json["snapshots"], serde_json::json!([]));
    let fv = json["format_version"].as_u64().expect("format_version u64");
    assert!(
        (1..=3).contains(&fv),
        "format_version in 1..=3; body={json}"
    );
}
