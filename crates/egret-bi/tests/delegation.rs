//! B3a Task 3: `resolve_delegation` — the `(session_user, bearer_token)` picker
//! that fronts the three object-scoped BI data endpoints. DB-gated; self-skips
//! without DATABASE_URL.
//!
//! Covers all three branches with a fake `EngineTokenMinter`:
//!  * DELEGATED connection + an OIDC user with a refresh token → mint succeeds →
//!    `(session_user = the user's username, bearer = the minted JWT)` — NOT the
//!    caller email, and a bearer IS attached.
//!  * DELEGATED connection + a non-OIDC user (mint yields `NoRefreshToken`) →
//!    the helper DENIES with a 403 (no silent service-cred fallback).
//!  * NON-DELEGATED connection → the current shared-service behavior is byte-
//!    identical: `(session_user = caller email, bearer = None)` — mint never runs.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use egret_bi::{BiState, ChartRepo, DashboardRepo, DatasetRepo, SavedQueryRepo, SchemaService};
use egret_metastore::Db;
use std::sync::Arc;
use uuid::Uuid;

/// Fake minter: `Some(token)` → mint ok; `None` → `NoRefreshToken` (a non-OIDC user).
struct FakeMinter {
    token: Option<String>,
}
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for FakeMinter {
    async fn mint(
        &self,
        _user: uuid::Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        match &self.token {
            Some(t) => Ok(egret_analytics_api::engine_token::EngineToken {
                access_token: t.clone(),
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            }),
            None => Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken),
        }
    }
}

/// The PDP is never consulted by `resolve_delegation`; this stub keeps `BiState`
/// buildable without a live Cedar snapshot.
struct AllowAll;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for AllowAll {
    async fn authorize(
        &self,
        _subject: &egret_core::authz::Subject,
        _action: &str,
        _resource: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: true,
            reasons: vec![],
        })
    }
}

fn build_state(
    db: Db,
    minter: Arc<dyn egret_analytics_api::engine_token::EngineTokenMinter>,
) -> BiState {
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(AllowAll);
    BiState {
        db: db.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: minter,
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

async fn seed_user(db: &Db, email: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(email)
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

async fn seed_db(db: &Db, ws: Uuid, delegated: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id, delegated) \
         VALUES ($1,'trino','localhost',8080,$2,$3) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .bind(delegated)
    .fetch_one(&db.pool)
    .await
    .expect("insert database")
}

fn ctx_for(ws: Uuid, uid: Uuid, email: &str) -> egret_core::WorkspaceContext {
    egret_core::WorkspaceContext {
        workspace_id: egret_core::WorkspaceId(ws),
        principal: egret_core::Principal {
            user_id: egret_core::UserId(uid),
            email: email.to_string(),
            platform_roles: vec![],
            permissions: vec![],
            group_ids: vec![],
        },
        workspace_role: "ws_viewer".into(),
    }
}

#[tokio::test]
async fn resolve_delegation_covers_all_three_branches() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let email = format!("deleg_{}@egret.local", Uuid::new_v4().simple());
    let uid = seed_user(&db, &email).await;
    let slug = format!("deleg-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Delegation Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let delegated_db = seed_db(&db, ws, true).await;
    let plain_db = seed_db(&db, ws, false).await;
    let ctx = ctx_for(ws, uid, &email);

    // The expected username the engine matches the JWT principal against: the
    // stable `preferred_username` derived+persisted by `ensure_username`.
    let users = egret_auth::users::UserRepo::new(db.clone());
    let expected_username = users
        .ensure_username(egret_core::UserId(uid))
        .await
        .expect("ensure_username");
    assert_ne!(
        expected_username, email,
        "sanity: username must differ from email so the assertions are meaningful"
    );

    // Branch 1: DELEGATED + a user with a refresh token → mint OK → username + JWT.
    let st_ok = build_state(
        db.clone(),
        Arc::new(FakeMinter {
            token: Some("fake-engine-jwt".into()),
        }),
    );
    let (session_user, bearer) = egret_bi::resolve_delegation(&st_ok, &ctx, delegated_db)
        .await
        .expect("delegated mint should succeed");
    assert_eq!(
        session_user,
        Some(expected_username.clone()),
        "delegated: session_user MUST be the username (preferred_username), not the email"
    );
    assert_eq!(
        bearer,
        Some("fake-engine-jwt".to_string()),
        "delegated: the minted per-user JWT MUST be attached as the bearer"
    );

    // Branch 2: DELEGATED + a non-OIDC user (no refresh token) → 403, no fallback.
    let st_none = build_state(db.clone(), Arc::new(FakeMinter { token: None }));
    let err = egret_bi::resolve_delegation(&st_none, &ctx, delegated_db)
        .await
        .expect_err("a non-OIDC user on a delegated connection must be denied");
    assert_eq!(
        err.into_response().status(),
        StatusCode::FORBIDDEN,
        "delegated + no refresh token → 403 (not a silent service-cred fallback)"
    );

    // Branch 3: NON-DELEGATED → byte-identical shared-service behavior (email, no bearer).
    let (session_user, bearer) = egret_bi::resolve_delegation(&st_ok, &ctx, plain_db)
        .await
        .expect("non-delegated must never fail on mint");
    assert_eq!(
        session_user,
        Some(email.clone()),
        "non-delegated: session_user stays the caller email"
    );
    assert_eq!(bearer, None, "non-delegated: NO bearer is attached");
}
