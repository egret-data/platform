//! The engine-mgmt seam router's shared state.

use axum::extract::FromRef;
use egret_auth::session::SessionService;
use egret_engine_mgmt_api::EngineProvisioningPort;
use std::sync::Arc;

use crate::EngineMgmtStore;

#[derive(Clone)]
pub struct EngineMgmtState {
    /// Its own data (E1b) — engines/engine_catalogs CRUD.
    pub engine_mgmt: EngineMgmtStore,
    /// Global storage-connection registry (`storage_connections`) — cross-seam:
    /// auto-wiring.
    pub storage: egret_catalog::CatalogStore,
    /// The Policy/Ranger seam, behind its contract `PolicyPort` (E2a) — cross-seam:
    /// engine-create auto-bind to a Ranger profile. The concrete `AuthzStore` is
    /// wired in the composition root; at a real split an HTTP client fills this in.
    pub authz: Arc<dyn egret_policy_api::PolicyPort>,
    /// Named secret layer, used in GLOBAL scope (`None` workspace) here —
    /// secret resolution.
    pub secrets: egret_secrets::WorkspaceSecretStore,
    /// Session service backing the `Auth` extractor.
    pub sessions: SessionService,
    /// Provisions/updates/deletes query engines on Kubernetes (kube-free
    /// Port); `None` in tests and when no cluster is configured (routes then
    /// report "kubernetes not configured").
    pub engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    /// Serves exec/pod-logs/coordinator-request directly (kube-shaped, not on
    /// the Port); `None` in tests and when no cluster is configured.
    pub kube: Option<Arc<dyn egret_orchestrator::KubeClient>>,
    /// In-cluster URL Trino pods use to reach egret's REST catalog + token
    /// endpoint (e.g. `http://egret:8080`); populated from config in egret-bin.
    pub engine_egret_url: String,
    /// Default JWKS URI for a Trino JWT authenticator when the engine's auth
    /// block omits one; populated from config in egret-bin. Tests set a
    /// dummy value.
    pub engine_jwks_uri: String,
}

impl FromRef<EngineMgmtState> for SessionService {
    fn from_ref(s: &EngineMgmtState) -> Self {
        s.sessions.clone()
    }
}
