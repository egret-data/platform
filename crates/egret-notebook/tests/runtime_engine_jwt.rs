//! F2-P1 T6: the notebook-runtime manifest carries a short-lived per-user engine
//! JWT minted via the injected `EngineTokenMinter`.

use std::sync::Arc;

use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::ids::WorkspaceId;
use egret_metastore::Db;
use egret_notebook::{build_notebook_runtime, NotebookState, NotebookStore};
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

/// Local fake minter — returns a canned engine JWT with a far-future expiry.
struct StubMinter;

#[async_trait::async_trait]
impl egret_notebook::engine_token::EngineTokenMinter for StubMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<egret_notebook::engine_token::EngineToken, egret_notebook::engine_token::BrokerError>
    {
        Ok(egret_notebook::engine_token::EngineToken {
            access_token: "stub.jwt".into(),
            expires_at: chrono::DateTime::<chrono::Utc>::from_timestamp(4102444800, 0).unwrap(),
        })
    }
}

/// Incidental PDP: these tests call `build_notebook_runtime` directly (not
/// the `runtime` HTTP handler), so `st.pdp` is never consulted — the field
/// only needs to exist to satisfy `NotebookState`'s constructor.
struct AllowAllPdp;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for AllowAllPdp {
    async fn authorize(
        &self,
        _s: &egret_core::authz::Subject,
        _a: &str,
        _r: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: true,
            reasons: vec![],
        })
    }
}

/// Minter for a user with no stored refresh token — always `NoRefreshToken`.
struct NoRefreshMinter;

#[async_trait::async_trait]
impl egret_notebook::engine_token::EngineTokenMinter for NoRefreshMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<egret_notebook::engine_token::EngineToken, egret_notebook::engine_token::BrokerError>
    {
        Err(egret_notebook::engine_token::BrokerError::NoRefreshToken)
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn runtime_manifest_carries_engine_jwt(pool: sqlx::PgPool) {
    let db = Db { pool };

    let users = UserRepo::new(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let owner = users
        .insert_local(&format!("rtjwt-{suffix}@egret.test"), "RT JWT", "x")
        .await
        .expect("seed user");

    let workspaces = WorkspaceRepo::new(db.clone());
    let ws = workspaces
        .create(&format!("rtjwt{suffix}"), "RT JWT WS", owner)
        .await
        .expect("seed ws");
    let ws_id: WorkspaceId = ws.id;

    let state = NotebookState {
        store: NotebookStore::new(db.clone()),
        catalog: egret_catalog_engine::CatalogStore::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces,
        authz: Authorizer::new(db.clone()),
        pdp: Arc::new(AllowAllPdp),
        users: UserRepo::new(db.clone()),
        catalog_base_url: "http://localhost:8181".into(),
        engine_minter: Arc::new(StubMinter),
    };

    let m = build_notebook_runtime(&state, ws_id.0, owner)
        .await
        .expect("build manifest");

    assert_eq!(m.engine_jwt.as_deref(), Some("stub.jwt"));
    assert!(m.engine_jwt_expires_at.unwrap() > chrono::Utc::now());
}

/// Regression guard: a user with no stored refresh token (the minter returns
/// `BrokerError::NoRefreshToken`) must NOT cause the manifest build to fail.
/// Minting is best-effort — the call succeeds with `engine_jwt == None`.
#[sqlx::test(migrations = "../../migrations")]
async fn runtime_manifest_no_refresh_token_is_ok(pool: sqlx::PgPool) {
    let db = Db { pool };

    let users = UserRepo::new(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let owner = users
        .insert_local(&format!("rtnort-{suffix}@egret.test"), "RT NoRT", "x")
        .await
        .expect("seed user");

    let workspaces = WorkspaceRepo::new(db.clone());
    let ws = workspaces
        .create(&format!("rtnort{suffix}"), "RT NoRT WS", owner)
        .await
        .expect("seed ws");
    let ws_id: WorkspaceId = ws.id;

    let state = NotebookState {
        store: NotebookStore::new(db.clone()),
        catalog: egret_catalog_engine::CatalogStore::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces,
        authz: Authorizer::new(db.clone()),
        pdp: Arc::new(AllowAllPdp),
        users: UserRepo::new(db.clone()),
        catalog_base_url: "http://localhost:8181".into(),
        engine_minter: Arc::new(NoRefreshMinter),
    };

    let m = build_notebook_runtime(&state, ws_id.0, owner)
        .await
        .expect("build manifest must succeed even without a refresh token");

    assert_eq!(m.engine_jwt, None);
    assert_eq!(m.engine_jwt_expires_at, None);
}
