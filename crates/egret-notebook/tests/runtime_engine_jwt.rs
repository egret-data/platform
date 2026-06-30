//! F2-P1 T6: the notebook-runtime manifest carries a short-lived per-user engine
//! JWT minted via the injected `EngineTokenMinter`.

use std::sync::Arc;

use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_core::ids::WorkspaceId;
use egret_metastore::Db;
use egret_notebook::error::NotebookApiError;
use egret_notebook::{build_notebook_runtime, NamespaceProvisioner, NotebookState, NotebookStore};
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

/// No lakekeeper in unit tests — always succeeds.
struct FakeProvisioner;

#[async_trait::async_trait]
impl NamespaceProvisioner for FakeProvisioner {
    async fn ensure_namespace(
        &self,
        _warehouse_id: Uuid,
        _namespace: &str,
    ) -> Result<(), NotebookApiError> {
        Ok(())
    }
}

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

    // A warehouse is required for provisioning the private namespace.
    sqlx::query(
        "INSERT INTO workspace_warehouses (workspace_id, warehouse_id, name) VALUES ($1, $2, $3)",
    )
    .bind(ws_id.0)
    .bind(Uuid::new_v4())
    .bind(format!("rtjwt-wh-{}", Uuid::new_v4().simple()))
    .execute(&db.pool)
    .await
    .expect("seed warehouse");

    let state = NotebookState {
        store: NotebookStore::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces,
        authz: Authorizer::new(db.clone()),
        catalog: CatalogStore::new(db.clone()),
        provisioner: Arc::new(FakeProvisioner),
        users: UserRepo::new(db.clone()),
        catalog_base_url: "http://localhost:8181".into(),
        engine_minter: Arc::new(StubMinter),
    };

    let m = build_notebook_runtime(&state, ws_id.0, owner)
        .await
        .expect("build manifest");

    assert_eq!(m.engine_jwt, "stub.jwt");
    assert!(m.engine_jwt_expires_at > chrono::Utc::now());
}
