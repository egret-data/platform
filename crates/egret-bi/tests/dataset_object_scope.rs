//! Per-object authorization for `datasets/{id}` handlers (Effort B / B2-1a Task 5).
//! DB-gated; self-skips without DATABASE_URL. Builds a REAL `CedarPdp` AFTER
//! `migrate()` so it snapshots the `ws_viewer`/`ws_editor` BI perms (0012 +
//! migration 0070).
//!
//! Proves the dataset per-item handlers are now object-scoped (`require_object`):
//! an OUTSIDE user (not a workspace member) with a `ws_viewer` object grant on a
//! dataset can READ it (200) but not a sibling (403) and cannot UPDATE it (403,
//! viewer lacks manage); an OUTSIDE user with a `ws_editor` object grant can
//! UPDATE the granted dataset (2xx) but not a sibling (403); a workspace
//! `ws_editor` member still reads + updates (does everything).

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

/// Test stub: object-scope tests use non-delegated DBs, so mint is never called.
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
    }
}

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

async fn session_for(sessions: &egret_auth::session::SessionService, uid: Uuid) -> String {
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
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

/// Seed a direct object grant of workspace `role` to `uid` on dataset `ds`.
async fn grant_object(db: &Db, uid: Uuid, ds: Uuid, role: &str, granted_by: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .expect("role id");
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ('user', $1, $2, 'object', 'dataset', $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(uid)
    .bind(role_id)
    .bind(ds)
    .bind(granted_by)
    .execute(&db.pool)
    .await
    .expect("seed object grant");
}

async fn get_dataset(app: &axum::Router, slug: &str, ds: Uuid, tok: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/datasets/{ds}"))
                .header("Cookie", format!("egret_session={tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// PUT a rename with a per-call-unique name (datasets are unique per
/// (workspace, name), so successful updates must not collide).
async fn update_dataset(
    app: &axum::Router,
    slug: &str,
    ds: Uuid,
    tok: &str,
    name: &str,
) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/datasets/{ds}"))
                .header("Cookie", format!("egret_session={tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(r#"{{"name":"{name}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn dataset_per_object_handlers_are_object_scoped() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // Sanity: ws_viewer holds dataset:read (migration 0070) but NOT dataset:manage;
    // ws_editor holds both (0012). This underpins read-yes / manage-no for viewers.
    let viewer_read: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
         WHERE r.scope = 'workspace' AND r.name = 'ws_viewer' AND rp.permission = 'dataset:read'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        viewer_read, 1,
        "ws_viewer must have dataset:read (migration 0070)"
    );
    let viewer_manage: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
         WHERE r.scope = 'workspace' AND r.name = 'ws_viewer' AND rp.permission = 'dataset:manage'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(viewer_manage, 0, "ws_viewer must NOT have dataset:manage");

    let owner = seed_user(&db).await;
    let slug = format!("dsobj-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DS Obj Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // datasets.database_id is NOT NULL — seed a minimal databases row.
    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // Principals: a ws_editor member (does everything), and TWO outside users
    // (not members) — one gets a Viewer object grant, one an Editor object grant.
    let editor_member = seed_user(&db).await;
    add_member(&db, ws, editor_member, "ws_editor").await;
    let outside_viewer = seed_user(&db).await;
    let outside_editor = seed_user(&db).await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();

    let editor_tok = session_for(&sessions, editor_member).await;
    let outside_viewer_tok = session_for(&sessions, outside_viewer).await;
    let outside_editor_tok = session_for(&sessions, outside_editor).await;

    // Two datasets in the workspace: DS (shared) and DS2 (sibling, not shared).
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
                data_type: "integer".into(),
                type_generic: "numeric".into(),
                is_temporal: false,
            }],
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;
    let ds2 = st
        .datasets
        .create(
            ws,
            db_id,
            "DS2",
            "",
            &SourceSpec::Virtual {
                sql: "SELECT 2".into(),
            },
            &[NewColumn {
                column_name: "two".into(),
                data_type: "integer".into(),
                type_generic: "numeric".into(),
                is_temporal: false,
            }],
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;

    // Direct object grants (seeded straight into access_grants).
    grant_object(&db, outside_viewer, ds, "ws_viewer", owner).await;
    grant_object(&db, outside_editor, ds, "ws_editor", owner).await;

    let app = dataset_router().with_state(st);

    // 1. Outside VIEWER: reads DS via object grant (200), denied sibling DS2 (403),
    //    denied update (viewer lacks dataset:manage → 403).
    let read_ds = get_dataset(&app, &slug, ds, &outside_viewer_tok).await;
    assert_eq!(
        read_ds,
        StatusCode::OK,
        "outside ws_viewer object-grant must READ its granted dataset DS (200)"
    );
    let read_sibling = get_dataset(&app, &slug, ds2, &outside_viewer_tok).await;
    assert_eq!(
        read_sibling,
        StatusCode::FORBIDDEN,
        "outside ws_viewer must NOT read ungranted sibling DS2 (403)"
    );
    let viewer_update = update_dataset(&app, &slug, ds, &outside_viewer_tok, "v_rename").await;
    assert_eq!(
        viewer_update,
        StatusCode::FORBIDDEN,
        "outside ws_viewer must NOT update DS (viewer lacks manage → 403)"
    );

    // 2. Outside EDITOR: updates the granted DS (2xx) but not the sibling DS2 (403).
    let editor_update_ds =
        update_dataset(&app, &slug, ds, &outside_editor_tok, "e_rename_ds").await;
    assert!(
        editor_update_ds.is_success(),
        "outside ws_editor object-grant must UPDATE its granted dataset DS (2xx); got {editor_update_ds}"
    );
    let editor_update_sibling =
        update_dataset(&app, &slug, ds2, &outside_editor_tok, "e_rename_ds2").await;
    assert_eq!(
        editor_update_sibling,
        StatusCode::FORBIDDEN,
        "outside ws_editor must NOT update ungranted sibling DS2 (403)"
    );

    // 3. Workspace ws_editor member still does everything (read + update, any dataset).
    let member_read = get_dataset(&app, &slug, ds, &editor_tok).await;
    assert_eq!(
        member_read,
        StatusCode::OK,
        "ws_editor member must read DS (200)"
    );
    let member_update = update_dataset(&app, &slug, ds2, &editor_tok, "m_rename_ds2").await;
    assert!(
        member_update.is_success(),
        "ws_editor member must update datasets (2xx); got {member_update}"
    );
}
