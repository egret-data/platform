//! Router smoke test — unauthenticated requests rejected.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    chart_router, dashboard_router, dataset_router, sqllab_router, BiState, ChartRepo,
    DashboardRepo, DatasetRepo, SavedQueryRepo, SchemaService,
};
use egret_metastore::Db;
use tower::ServiceExt;

fn build_state(db: Db) -> BiState {
    BiState {
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
        charts: ChartRepo::new(db.clone()),
        datasets: DatasetRepo::new(db.clone()),
        dashboards: DashboardRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db),
    }
}

async fn state() -> Option<BiState> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(build_state(db))
}

#[tokio::test]
async fn saved_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = sqllab_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/sqllab/queries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn schema_catalogs_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = sqllab_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/sqllab/databases/00000000-0000-0000-0000-000000000000/catalogs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dataset_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = dataset_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/datasets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn chart_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = chart_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/charts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dashboard_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = dashboard_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/dashboards")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
