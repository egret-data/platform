//! Router smoke tests via oneshot. Unauthenticated requests must be rejected.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_database::{admin_database_router, me_database_router, DatabaseRepo, DatabaseState};
use egret_metastore::Db;
use egret_secrets::SecretStore;
use tower::ServiceExt;

async fn state() -> Option<DatabaseState> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(DatabaseState {
        repo: DatabaseRepo::new(db.clone(), SecretStore::new(db.clone(), None)),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db),
    })
}

#[tokio::test]
async fn admin_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = admin_database_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/databases")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = me_database_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/databases?workspace=eng")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
