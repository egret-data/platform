//! Router smoke tests. Unauthenticated requests must be rejected.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_engine::{engine_router, EngineState};
use tower::ServiceExt;

async fn state() -> Option<EngineState> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    Some(EngineState::for_test(db))
}

#[tokio::test]
async fn submit_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = engine_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ws/eng/queries")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"database_id":"00000000-0000-0000-0000-000000000000","sql":"SELECT 1"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn history_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = engine_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/queries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
