//! Object-scoped `POST /ws/{ws}/dashboards/{id}/cards/{card_id}/data`
//! (Effort B / B2-1b Task 3). DB-gated; self-skips without DATABASE_URL. Builds
//! a REAL `CedarPdp` AFTER `migrate()` so it snapshots the `ws_viewer` BI perms.
//!
//! Proves the card-data endpoint authorizes on the DASHBOARD, not the chart —
//! a dashboard grant TRANSITIVELY authorizes its cards' data (no per-chart grant):
//!  * an OUTSIDE user (not a workspace member) with a `ws_viewer` object grant on
//!    dashboard D — and NO grant on the card's chart — REACHES execution on
//!    `.../cards/{card}/data` (200 with a live engine, else 502) — TRANSITIVE auth;
//!  * a `card_id` that does NOT belong to D (a card from another dashboard, and a
//!    random uuid) → 404, even for the dashboard-granted viewer;
//!  * a user with NO grant on D → 403, BEFORE any card lookup or execution.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    dashboard_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, NewColumn, SavedQueryRepo,
    SchemaService, SourceSpec,
};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> BiState {
    BiState {
        db: db.clone(),
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

/// Seed a direct object grant of workspace `role` to `uid` on `object_type`/`obj`.
async fn grant_object(db: &Db, uid: Uuid, object_type: &str, obj: Uuid, role: &str, by: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .expect("role id");
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ('user', $1, $2, 'object', $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(uid)
    .bind(role_id)
    .bind(object_type)
    .bind(obj)
    .bind(by)
    .execute(&db.pool)
    .await
    .expect("seed object grant");
}

/// Insert a chart-backed card onto `dashboard_id`, returning the new card id.
async fn seed_card(db: &Db, dashboard_id: Uuid, chart_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO analytics.dashboard_cards \
           (dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal) \
         VALUES ($1, $2, 'chart', 0, 0, 4, 4, '{}'::jsonb, 0) RETURNING id",
    )
    .bind(dashboard_id)
    .bind(chart_id)
    .fetch_one(&db.pool)
    .await
    .expect("insert card")
}

async fn post_card_data(
    app: &axum::Router,
    slug: &str,
    dash: Uuid,
    card: Uuid,
    tok: &str,
) -> (StatusCode, Vec<u8>) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/ws/{slug}/dashboards/{dash}/cards/{card}/data"))
                .header("Cookie", format!("egret_session={tok}"))
                .header("Content-Type", "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn dashboard_card_data_is_transitively_object_scoped() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let slug = format!("dbcard-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DB Card Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // charts.dataset_id → a dataset, which needs a (granted) database connection.
    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // One OUTSIDE user (not a workspace member) who will get a Viewer object grant
    // on the DASHBOARD only (never on the chart).
    let outside_viewer = seed_user(&db).await;
    // A second outside user with NO grant at all.
    let no_grant = seed_user(&db).await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let outside_viewer_tok = session_for(&sessions, outside_viewer).await;
    let no_grant_tok = session_for(&sessions, no_grant).await;

    // A backing dataset + saved chart spec (as the frontend persists it).
    let ds = st
        .datasets
        .create(
            ws,
            db_id,
            "DS",
            "",
            &SourceSpec::Virtual {
                sql: "SELECT 1 AS one".into(),
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
    let spec = serde_json::json!({
        "metrics": [],
        "dimensions": [],
        "columns": ["one"],
        "filters": [],
        "orderby": [],
        "row_limit": 10,
        "rollup_groups": []
    });
    let params = serde_json::json!({ "spec": spec });
    let chart = st
        .charts
        .create(ws, ds, "CH", "", "table", &params, Some(owner))
        .await
        .unwrap()
        .id;

    // Dashboard D holds a card referencing the chart. The outside viewer is granted
    // on D ONLY — never on the chart — to prove transitive auth.
    let dash = st
        .dashboards
        .create(ws, "D", "", Some(owner))
        .await
        .unwrap()
        .id;
    let card = seed_card(&db, dash, chart).await;

    // A SECOND dashboard + its own card, to test a card that isn't on D.
    let other_dash = st
        .dashboards
        .create(ws, "OTHER", "", Some(owner))
        .await
        .unwrap()
        .id;
    let foreign_card = seed_card(&db, other_dash, chart).await;

    grant_object(&db, outside_viewer, "dashboard", dash, "ws_viewer", owner).await;

    let app = dashboard_router().with_state(st);

    // 1. TRANSITIVE auth: the outside viewer has a grant on D and NO grant on the
    //    card's chart, yet REACHES execution on the card's data (200 with a live
    //    engine, else 502 — engine reached but unavailable in CI). Never 403/404.
    let (status, body) = post_card_data(&app, &slug, dash, card, &outside_viewer_tok).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_GATEWAY,
        "dashboard grant must TRANSITIVELY reach card execution (200 with a live \
         engine, else 502); got {status}, body={}",
        String::from_utf8_lossy(&body)
    );
    if status == StatusCode::OK {
        let v: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert!(
            v.get("columns")
                .and_then(|c| c.as_array())
                .is_some_and(|c| !c.is_empty()),
            "ChartRunResponse must carry columns; got {v}"
        );
        assert!(
            v.get("rows").and_then(|r| r.as_array()).is_some(),
            "must carry rows"
        );
    } else {
        eprintln!("note: no live query engine — transitive authz reached run_query (502)");
    }

    // 2a. A card that belongs to ANOTHER dashboard → 404 (not on D), even though
    //     the viewer is granted on D.
    let (foreign_status, _) =
        post_card_data(&app, &slug, dash, foreign_card, &outside_viewer_tok).await;
    assert_eq!(
        foreign_status,
        StatusCode::NOT_FOUND,
        "a card from another dashboard is not on D → 404"
    );

    // 2b. A random card_id → 404.
    let (random_status, _) =
        post_card_data(&app, &slug, dash, Uuid::new_v4(), &outside_viewer_tok).await;
    assert_eq!(
        random_status,
        StatusCode::NOT_FOUND,
        "an unknown card_id → 404"
    );

    // 3. A user with NO grant on D → 403, BEFORE any card lookup or execution.
    let (denied_status, _) = post_card_data(&app, &slug, dash, card, &no_grant_tok).await;
    assert_eq!(
        denied_status,
        StatusCode::FORBIDDEN,
        "a user with no grant on the dashboard is denied (403)"
    );
}
