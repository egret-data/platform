//! The `semantic_type` allow-list, end to end. DB-gated; self-skips without DATABASE_URL.
//!
//! WHY THIS EXISTS AS A ROUTE TEST AND NOT ONLY AS UNIT TESTS. `validate_semantic_type` has its own
//! unit tests in `egret-analytics-api`, and they cover the vocabulary thoroughly. What they cannot
//! cover is whether anything CALLS it: delete the four-line guard from `ds_update_column` and every
//! one of those unit tests still passes. That is precisely the shape of the original defect — the
//! vocabulary was written down (in a `<select>`) and nothing enforced it — so a test suite that
//! proves the rule exists but not that it is applied would repeat the mistake one layer up.
//! Confirmed by mutation: removing the handler's guard leaves the unit tests green and turns THIS
//! file red.
//!
//! The second test covers the other half of the defence. The handler is today's only HTTP writer,
//! but "today's only writer" is a fact with a short shelf life, and migration 0104's CHECK is what
//! makes the guarantee a property of the data rather than of one code path. It is asserted here
//! rather than assumed because a migration that is written and never run is indistinguishable from
//! one that was never written.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    dataset_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, NewColumn, SavedQueryRepo,
    SchemaService, SourceSpec,
};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct NoMinter;
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: uuid::Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> BiState {
    BiState {
        db: db.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
        saved: SavedQueryRepo::new(db.clone()),
        schema: SchemaService::new(),
        databases: egret_query::DatabaseRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
            egret_secrets::WorkspaceSecretStore::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
        ),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        charts: ChartRepo::new(db.clone()),
        datasets: DatasetRepo::new(db.clone()),
        dashboards: DashboardRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db),
        audit: egret_audit::AuditSink::new(16).0,
    }
}

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

async fn add_member(db: &Db, ws: Uuid, uid: Uuid, role: &str) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3 ON CONFLICT DO NOTHING",
    )
    .bind(ws)
    .bind(uid)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("add member");
}

/// PUT one `semantic_type` onto a column, exactly as the Role `<select>` does.
async fn put_semantic_type(
    app: &axum::Router,
    slug: &str,
    ds: Uuid,
    tok: &str,
    value: &str,
) -> (StatusCode, String) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/datasets/{ds}/columns/one"))
                .header("Cookie", format!("egret_session={tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "semantic_type": value }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn semantic_type_writes_are_checked_against_the_allow_list() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let slug = format!("dssem-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DS Semantic Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    add_member(&db, ws, owner, "ws_editor").await;

    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let tok = st
        .sessions
        .create(egret_core::ids::UserId(owner), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    let ds = st
        .datasets
        .create(
            ws,
            db_id,
            "DS",
            "",
            &SourceSpec::Virtual {
                sql: "SELECT 1".into(),
            },
            &[NewColumn {
                column_name: "one".into(),
                data_type: "double".into(),
                type_generic: "numeric".into(),
                is_temporal: false,
            }],
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;

    let app = dataset_router().with_state(st);

    // 1. Every value the Role select offers is accepted. A rejection here would be worse than the
    //    original defect: the editor would offer a choice the API refuses.
    for v in egret_analytics_api::bi::SEMANTIC_TYPES {
        let (status, body) = put_semantic_type(&app, &slug, ds, &tok, v).await;
        assert!(
            status.is_success(),
            "the Role select offers {v:?}; API answered {status}: {body}"
        );
    }

    // 2. The defect #438 measured: `PUT semantic_type: "banana"` used to persist.
    let (status, body) = put_semantic_type(&app, &slug, ds, &tok, "banana").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(
        body.contains("banana") && body.contains("latitude"),
        "the 400 should quote the value and list the permitted ones: {body}"
    );

    // 3. The near-miss that made this stop being harmless. `map.tsx` compares `=== "latitude"`, so
    //    "Latitude" would store cleanly and then produce a picker that never fills.
    for v in ["Latitude", "lat", "type/Latitude", " latitude"] {
        let (status, _) = put_semantic_type(&app, &slug, ds, &tok, v).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{v:?} must be rejected");
    }

    // 4. The rejections did not partially apply: the column still holds the last GOOD value, which
    //    is `longitude` (the final entry of SEMANTIC_TYPES).
    let stored: String = sqlx::query_scalar(
        "SELECT semantic_type FROM analytics.dataset_columns WHERE dataset_id = $1 AND column_name = 'one'",
    )
    .bind(ds)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(stored, "longitude");
}

#[tokio::test]
async fn migration_0104_constrains_the_column_in_the_database() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // The constraint exists under the name 0104 gives it. Asserted by name so that a future
    // migration dropping and recreating it under a different name is a visible change here.
    let present: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_constraint WHERE conname = 'dataset_columns_semantic_type_check'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(present, 1, "migration 0104's CHECK is missing");

    // And it bites, for a writer that never goes through the handler. This is the whole point of
    // having it as well as the API guard.
    let err = sqlx::query(
        "INSERT INTO analytics.dataset_columns \
           (dataset_id, column_name, data_type, type_generic, is_temporal, ordinal, semantic_type) \
         VALUES (gen_random_uuid(), 'c', 'double', 'numeric', false, 0, 'banana')",
    )
    .execute(&db.pool)
    .await;
    let msg = err
        .expect_err("a direct INSERT of an unknown role must be refused")
        .to_string();
    assert!(
        msg.contains("dataset_columns_semantic_type_check"),
        "expected the semantic_type CHECK to reject it, got: {msg}"
    );
}
