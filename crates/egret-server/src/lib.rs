//! HTTP composition root: shared application state, the router, and health endpoints.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::routes::{auth_router, AuthState};
use egret_metastore::Db;
use egret_workspace::routes::{admin_workspace_router, WorkspaceState};

/// Application state shared by all handlers.
#[derive(Clone)]
pub struct AppState {
    /// `None` in unit tests; `Some` once a database is wired in `egret-bin`.
    pub db: Option<Db>,
    /// Present once a database is wired (in `egret-bin`); `None` in pure health unit tests.
    pub auth: Option<AuthState>,
    /// Present once workspace crate is wired; `None` in pure health unit tests.
    pub workspace: Option<WorkspaceState>,
}

impl AppState {
    /// Build state with no database — used by unit tests and `--no-db` boots.
    pub fn without_db() -> Self {
        Self {
            db: None,
            auth: None,
            workspace: None,
        }
    }
}

/// Build the root router. Later phases mount feature routers here.
pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz));

    if let Some(auth) = state.auth.clone() {
        router = router.merge(
            Router::new()
                .nest("/api/auth", auth_router())
                .with_state(auth),
        );
    }

    if let Some(ws) = state.workspace.clone() {
        let admin = Router::new()
            .nest("/api/admin", admin_workspace_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(ws);
        router = router.merge(admin);
    }

    router.with_state(state)
}

/// Liveness: the process is up. Always 200.
async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Readiness: dependencies are reachable. 200 when the DB (if any) responds.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match &state.db {
        None => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "ok", "db": "none" })),
        ),
        Some(db) => match db.ping().await {
            Ok(()) => (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "ok", "db": "up" })),
            ),
            Err(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "degraded", "db": "down" })),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn healthz_returns_200_ok() {
        let app = build_router(AppState::without_db());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn readyz_ok_without_db() {
        let app = build_router(AppState::without_db());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
