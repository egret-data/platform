//! Governance SP2b Task 10: the OPA decision surface as `build_router` mounts
//! it, and the gate that decides whether it is mounted and refreshed.
//!
//! - With `AppState::opa` set, the five routes answer through `opa_router`
//!   (the OPA listener); `build_router` (the API port) never serves them (404),
//!   and with it `None` (the gate off) there is no OPA router at all.
//! - The request body is bounded at [`OPA_BODY_LIMIT_BYTES`] (4 MiB): one byte
//!   over is refused with a non-2xx (the plugin fails the query), never answered.
//! - [`start_opa`] returns nothing and touches no database while the gate is
//!   off; with it on, the refresher runs, survives refresh errors and stops at
//!   shutdown.
//! - Once through the real `DbVerifier` with a minted engine credential
//!   (DB-gated: `DATABASE_URL`, fails unless `EGRET_ALLOW_DB_SKIP` is set).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use egret_engine_mgmt_api::credential::{EngineCredentialVerifier, EngineIdentity};
use egret_perm_compile::opa::cache::{SnapshotCache, MAX_UNREAD, REFRESH_PERIOD};
use egret_perm_compile::opa::surface::{
    in_flight_gate, ParseGate, VerifiedCache, OPA_BODY_LIMIT_BYTES,
    OPA_DEFAULT_BATCH_BODY_LIMIT_BYTES, OPA_DEFAULT_BODY_READ_TIMEOUT,
};
use egret_perm_compile::{start_opa, OpaState, OPA_SURFACE_PREFIX};
use egret_perm_eval::{Snapshot, SnapshotInput};
use egret_perm_ir::audience::{Audience, Member, MemberName, Via};
use egret_perm_ir::statement::*;
use egret_server::{build_router, opa_router, AppState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const ENGINE: uuid::Uuid = uuid::Uuid::from_u128(1);
const RIGHT: &str = "right";

/// Accepts only `(ENGINE, RIGHT)` when `accept`; otherwise refuses everything.
struct Stub {
    accept: bool,
}

#[async_trait::async_trait]
impl EngineCredentialVerifier for Stub {
    async fn verify(&self, id: uuid::Uuid, pw: &str) -> Result<Option<EngineIdentity>, String> {
        Ok(
            (self.accept && id == ENGINE && pw == RIGHT).then_some(EngineIdentity {
                engine_id: id,
                authz_service_id: Some(1),
            }),
        )
    }
}

/// `ann` may read everything in catalog `lake`.
fn snapshot() -> Snapshot {
    Snapshot::build(SnapshotInput {
        service_id: 1,
        key: (1, 1),
        statements: vec![Statement {
            source: Source::Grant {
                grant_id: uuid::Uuid::from_u128(1),
            },
            audience: Audience {
                include: vec![Member {
                    name: MemberName::User("ann".into()),
                    via: Via::Direct,
                }],
                except: vec![],
            },
            target: Target::Path(Path {
                catalog_id: None,
                catalog: "lake".into(),
                schema: Level::Unnamed,
                table: Level::Unnamed,
                column: Level::Unnamed,
            }),
            effect: Effect::Allow {
                privileges: vec![Privilege::Read],
            },
            derivation: Derivation::AsDeclared,
        }],
        classified: vec![],
        lineage_principal: "egret-lineage".into(),
    })
    .expect("snapshot")
}

/// `ENGINE` bound to profile 1, which is Ready with [`snapshot`].
fn opa_state(verifier: Arc<dyn EngineCredentialVerifier>, engine: uuid::Uuid) -> OpaState {
    let cache = Arc::new(SnapshotCache::default());
    cache.insert_for_test(1, snapshot());
    cache.bind_for_test(engine, Some(1));
    OpaState {
        cache,
        verifier,
        credentials: Arc::new(VerifiedCache::new(Duration::from_secs(5))),
        parses: Arc::new(ParseGate::default()),
        in_flight: Arc::new(in_flight_gate()),
        batch_body_limit: OPA_DEFAULT_BATCH_BODY_LIMIT_BYTES,
        body_read_timeout: OPA_DEFAULT_BODY_READ_TIMEOUT,
    }
}

fn state_with(opa: Option<OpaState>) -> AppState {
    let mut state = AppState::without_db();
    state.opa = opa;
    state
}

/// The OPA listener's router (`None` with the gate off).
fn router(opa: Option<OpaState>) -> axum::Router {
    opa_router(&state_with(opa)).expect("the gate is on")
}

fn body(engine: uuid::Uuid, secret: &str, action: Value) -> Value {
    json!({"input": {"context": {
        "identity": {"user": "ann", "groups": []},
        "softwareStack": {"trinoVersion": "483"},
        "properties": {"egret.engine": engine.to_string(), "egret.secret": secret}},
        "action": action}})
}

fn select_lake() -> Value {
    json!({"operation": "SelectFromColumns", "resource":
        {"table": {"catalogName": "lake", "schemaName": "s", "tableName": "t", "columns": ["c"]}}})
}

async fn send_raw(app: &axum::Router, method: &str, route: &str, raw: String) -> (u16, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("{OPA_SURFACE_PREFIX}{route}"))
                .header("content-type", "application/json")
                .body(Body::from(raw))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status().as_u16();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    let v = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, v)
}

async fn send(app: &axum::Router, route: &str, body: Value) -> (u16, Value) {
    send_raw(app, "POST", route, body.to_string()).await
}

/// A request that would be allowed, padded with trailing JSON whitespace to
/// exactly `len` bytes (still valid JSON).
fn padded_allow(len: usize) -> String {
    let mut raw = body(ENGINE, RIGHT, select_lake()).to_string();
    assert!(raw.len() <= len);
    raw.push_str(&" ".repeat(len - raw.len()));
    raw
}

#[tokio::test]
async fn the_mounted_surface_answers_a_refused_engine_with_denials() {
    // Arrange
    let app = router(Some(opa_state(Arc::new(Stub { accept: false }), ENGINE)));

    // Act
    let allow = send(&app, "/allow", body(ENGINE, RIGHT, select_lake())).await;
    let get = send_raw(&app, "GET", "/allow", String::new()).await;
    let batch = send(
        &app,
        "/batch",
        body(
            ENGINE,
            RIGHT,
            json!({"operation": "FilterCatalogs", "filterResources": [{"catalog": {"name": "lake"}}]}),
        ),
    )
    .await;
    let rows = send(
        &app,
        "/row_filters",
        body(
            ENGINE,
            RIGHT,
            json!({"operation": "GetRowFilters", "resource":
                {"table": {"catalogName": "lake", "schemaName": "s", "tableName": "t"}}}),
        ),
    )
    .await;

    // Assert
    assert_eq!(allow, (200, json!({"result": false})));
    assert_eq!(get.0, 405);
    assert_eq!(batch, (200, json!({"result": []})));
    assert_eq!(rows.0, 403, "{rows:?}");
}

#[tokio::test]
async fn the_mounted_surface_answers_an_admitted_engine() {
    // Arrange: the positive control for the denials above.
    let app = router(Some(opa_state(Arc::new(Stub { accept: true }), ENGINE)));

    // Act
    let allow = send(&app, "/allow", body(ENGINE, RIGHT, select_lake())).await;

    // Assert
    assert_eq!(allow, (200, json!({"result": true})));
}

#[test]
fn with_the_gate_off_there_is_no_opa_router() {
    assert!(opa_router(&state_with(None)).is_none());
}

#[tokio::test]
async fn the_api_router_never_serves_the_opa_surface() {
    // Arrange: the gate is on, yet the API port must not carry the surface.
    let app = build_router(state_with(Some(opa_state(
        Arc::new(Stub { accept: true }),
        ENGINE,
    ))));

    // Act
    let allow = send(&app, "/allow", body(ENGINE, RIGHT, select_lake())).await;

    // Assert
    assert_eq!(allow.0, 404, "{allow:?}");
}

#[tokio::test]
async fn a_body_at_the_limit_is_answered_and_one_byte_over_is_refused() {
    // Arrange: an engine whose request would be allowed.
    let app = router(Some(opa_state(Arc::new(Stub { accept: true }), ENGINE)));

    // Act
    let at = send_raw(&app, "POST", "/allow", padded_allow(OPA_BODY_LIMIT_BYTES)).await;
    let over = send_raw(
        &app,
        "POST",
        "/allow",
        padded_allow(OPA_BODY_LIMIT_BYTES + 1),
    )
    .await;

    // Assert
    assert_eq!(at, (200, json!({"result": true})));
    assert_eq!(over.0, 413, "{over:?}");
}

#[test]
fn the_body_limit_is_four_mebibytes() {
    assert_eq!(OPA_BODY_LIMIT_BYTES, 4 * 1024 * 1024);
}

// ---- the gate and the refresher ----

#[test]
fn the_refresher_runs_every_second() {
    // `MAX_UNREAD = 30` means "about 30 s" only at this period.
    assert_eq!(REFRESH_PERIOD, Duration::from_secs(1));
    assert_eq!(MAX_UNREAD, 30);
}

fn stub() -> Option<Arc<dyn EngineCredentialVerifier>> {
    Some(Arc::new(Stub { accept: false }))
}

/// A pool whose every acquire fails fast: nothing listens on port 1.
fn unreachable_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://egret:egret@127.0.0.1:1/egret")
        .expect("lazy pool")
}

#[tokio::test]
async fn with_the_gate_off_nothing_starts_and_no_connection_is_opened() {
    // Arrange
    let Some(url) = egret_test_db::url().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy(&url)
        .expect("lazy pool");

    // Act
    let started = start_opa(
        false,
        OPA_DEFAULT_BATCH_BODY_LIMIT_BYTES,
        OPA_DEFAULT_BODY_READ_TIMEOUT,
        stub(),
        pool.clone(),
        std::future::pending(),
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Assert
    assert!(started.is_none());
    assert_eq!(pool.size(), 0, "a refresher ran with the gate off");
}

#[tokio::test]
async fn with_the_gate_on_the_refresher_reads_the_database() {
    // Arrange
    let Some(url) = egret_test_db::url().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy(&url)
        .expect("lazy pool");
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();

    // Act
    let (_state, handle) = start_opa(
        true,
        OPA_DEFAULT_BATCH_BODY_LIMIT_BYTES,
        OPA_DEFAULT_BODY_READ_TIMEOUT,
        stub(),
        pool.clone(),
        async {
            let _ = stopped.await;
        },
    )
    .expect("the gate is on");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while pool.size() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Assert
    assert!(pool.size() > 0, "the refresher never read the database");
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("the refresher stops at shutdown")
        .expect("no panic");
}

#[tokio::test]
async fn without_the_ranger_verifier_nothing_starts() {
    // Act: the gate on, but the Ranger surface (whose verifier OPA shares) off.
    let started = start_opa(
        true,
        OPA_DEFAULT_BATCH_BODY_LIMIT_BYTES,
        OPA_DEFAULT_BODY_READ_TIMEOUT,
        None,
        unreachable_pool(),
        std::future::pending(),
    );

    // Assert
    assert!(started.is_none());
}

#[tokio::test]
async fn a_failing_refresh_never_stops_the_refresher_but_shutdown_does() {
    // Arrange
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (_state, handle) = start_opa(
        true,
        OPA_DEFAULT_BATCH_BODY_LIMIT_BYTES,
        OPA_DEFAULT_BODY_READ_TIMEOUT,
        stub(),
        unreachable_pool(),
        async {
            let _ = stopped.await;
        },
    )
    .expect("the gate is on");

    // Act: the first refresh (at once) fails within its 200 ms acquire timeout.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let running_after_errors = !handle.is_finished();
    let _ = stop.send(());
    let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;

    // Assert
    assert!(
        running_after_errors,
        "a refresh error stopped the refresher"
    );
    joined
        .expect("the refresher stops at shutdown")
        .expect("no panic");
}

// ---- the real verifier ----

#[tokio::test]
async fn the_real_db_verifier_admits_a_minted_credential_and_refuses_another() {
    // Arrange
    let Some(url) = egret_test_db::url().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("test crypto");
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let store = egret_engine_mgmt::EngineMgmtStore::new(db.clone());
    let engine = store
        .create_engine(
            &format!("opa-mount-{}", uuid::Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            &json!({}),
            None,
        )
        .await
        .expect("seed engine");
    let creds = egret_engine_mgmt::ranger_credential::RangerCredentialStore::new(store, secrets);
    let minted = creds.ensure(&engine).await.expect("mint credential");
    let verifier = Arc::new(egret_engine_mgmt::ranger_credential::DbVerifier::new(creds));
    let app = router(Some(opa_state(verifier, engine.id)));

    // Act
    let right = send(
        &app,
        "/allow",
        body(engine.id, &minted.password, select_lake()),
    )
    .await;
    let wrong = send(
        &app,
        "/allow",
        body(engine.id, "not-the-secret", select_lake()),
    )
    .await;

    // Assert
    assert_eq!(right, (200, json!({"result": true})));
    assert_eq!(wrong, (200, json!({"result": false})));
}
