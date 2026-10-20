//! The engine-mgmt seam router's shared state.

use axum::extract::FromRef;
use egret_auth::session::SessionService;
use egret_engine_mgmt_api::EngineProvisioningPort;
use std::sync::Arc;

use crate::{EngineAssignmentService, EngineMgmtStore};

/// Whether egret's `/ranger` plugin surface requires a credential: this
/// crate's mirror of `egret_config::RangerAuthMode` (the seam does not depend
/// on the config crate; the composition root maps one onto the other).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RangerSurfaceMode {
    /// Every request needs an engine Basic credential or the operator bearer.
    #[default]
    Required,
    /// Anyone who can reach the surface is served (chosen explicitly).
    Anonymous,
}

#[derive(Clone)]
pub struct EngineMgmtState {
    /// Its own data (E1b) — engines/engine_catalogs CRUD.
    pub engine_mgmt: EngineMgmtStore,
    /// Orchestrates engine↔workspace assignment across the engine-provisioning
    /// and query seams (materialize/remove the read-only delegated connection).
    /// Backs the CP `PUT /engines/{id}/workspaces` and workspace-admin
    /// `/workspaces/{id}/engines` assignment endpoints (P1).
    pub assignment: EngineAssignmentService,
    /// Catalog RBAC (roles/grants/clients) + warehouse lookups, behind its
    /// contract `CatalogPort` (E3b) — cross-seam: engine-create auto-wiring of a
    /// Catalog Client. The concrete `CatalogStore` is wired in the composition
    /// root; at a real split an HTTP client fills this in.
    pub catalog: Arc<dyn egret_catalog_api::CatalogPort>,
    /// Global storage-connection registry (`storage.storage_connections`, E3a)
    /// — cross-seam: resolving S3 coords for provisioning.
    pub storage_registry: egret_storage::StorageRegistry,
    /// The Policy/Ranger seam, behind its contract `PolicyPort` (E2a) — cross-seam:
    /// engine-create auto-bind to a Ranger profile. The concrete `PolicyStore` is
    /// wired in the composition root; at a real split an HTTP client fills this in.
    pub authz: Arc<dyn egret_policy_api::PolicyPort>,
    /// Named secret layer, used in GLOBAL scope (`None` workspace) here —
    /// secret resolution.
    pub secrets: egret_secrets::WorkspaceSecretStore,
    /// Session service backing the `Auth` extractor.
    pub sessions: SessionService,
    /// User identity repo — resolves platform-admin `UserId`s to engine
    /// usernames when auto-seeding Ranger admin all-access on provision.
    pub users: egret_auth::users::UserRepo,
    /// Role repo — lists platform-admin `UserId`s for the Ranger auto-admin
    /// seed on governed-engine provision.
    pub roles: egret_auth::roles::RoleRepo,
    /// PDP-backed policy decision point for the seam's authz gates
    /// (`egret_core::authz::require`); platform_admin bypasses. Injected from
    /// the shared PDP in the composition root.
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    /// Provisions/updates/deletes query engines on Kubernetes (kube-free
    /// Port); `None` in tests and when no cluster is configured (routes then
    /// report "kubernetes not configured").
    pub engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    /// Serves exec/pod-logs/coordinator-request directly (kube-shaped, not on
    /// the Port); `None` in tests and when no cluster is configured.
    pub kube: Option<Arc<dyn egret_provisioner::KubeClient>>,
    /// In-cluster URL Trino pods use to reach egret's REST catalog + token
    /// endpoint (e.g. `http://egret:8080`); populated from config in egret-bin.
    pub engine_egret_url: String,
    /// Default JWKS URI for a Trino JWT authenticator when the engine's auth
    /// block omits one; populated from config in egret-bin. Tests set a
    /// dummy value.
    pub engine_jwks_uri: String,
    /// The raw OIDC issuer URL (e.g. a Keycloak realm URL), populated from
    /// config in egret-bin. Used to derive the `required_issuer` /
    /// `jwks_url` a StarRocks `CREATE SECURITY INTEGRATION` needs (StarRocks
    /// has no per-engine JWT-auth override surface like Trino's
    /// `trinoConfig.auth.jwt.jwksUri`, so its OIDC coordinates are always
    /// derived from this platform-wide default). Empty when OIDC is
    /// unconfigured — not hard-failed; a StarRocks jwt-delegation engine
    /// simply gets an empty issuer/jwks in that case. Tests set a dummy value.
    pub engine_oidc_issuer: String,
    /// Whether the platform Ranger integration is enabled (`config.ranger.enabled`).
    /// Gates the auto-CREATE of a Ranger policy profile on Trino engine create:
    /// when `false` (the default) a new Trino engine is left UNBOUND
    /// (`authz_service_id = None`) so the provisioner writes no
    /// `access-control.properties` and Trino defaults to allow-all — which is what
    /// makes per-user delegated impersonation queries work. An admin explicitly
    /// passing `authz_service_id` still binds regardless of this flag.
    pub ranger_enabled: bool,
    /// The mode egret's `/ranger` plugin surface runs in (`config.ranger.auth_mode`).
    /// The external plugin-config page states it: `Required` means the
    /// engine's plugin must present its Ranger credential.
    pub ranger_surface_mode: RangerSurfaceMode,
    /// The JDBC drivers a StarRocks `jdbc` catalog makes the FE download
    /// (design D5): the compiled-in pinned table with this deployment's
    /// `[starrocks.jdbc_drivers]` overrides applied. Populated from config in
    /// egret-bin; tests use `JdbcDrivers::default()`.
    pub sr_jdbc_drivers: egret_catalog_registry::JdbcDrivers,
    /// Runs statements on a StarRocks FE. A real `MysqlDdlRunner` in egret-bin;
    /// a fake in tests, which is what makes the bootstrap and the apply path
    /// testable at all (design D8).
    pub starrocks_ddl: Arc<dyn crate::starrocks_ddl::StarRocksDdl>,
    /// Brings a newly bound policy profile's compiled `perm` policies up to
    /// date (SP4a Task 9 review). `None` leaves the behaviour exactly as it was
    /// — the profile stays stale until someone calls the manual compile
    /// trigger — which is what every test that does not care about it wants.
    /// The concrete implementation is wired in the composition root; see
    /// [`crate::profile_compiler::ProfileCompilerPort`] for why it is a port
    /// and not a direct call.
    pub perm_compiler: Option<Arc<dyn crate::profile_compiler::ProfileCompilerPort>>,
    /// `egret-catalog-registry`'s narrower port (SP4a Task 16, D3 defect 2):
    /// `ensure_warehouse_catalog_with_compiler`'s recompile, threaded through
    /// this crate's two production callers of it
    /// (`create_trino_engine`/`create_starrocks_engine`). A DIFFERENT trait
    /// object than `perm_compiler` above — `egret-catalog-registry` cannot
    /// depend on this crate's own `ProfileCompilerPort` (this crate depends on
    /// `egret-catalog-registry`, not the other way around) — backed by the
    /// SAME concrete compiler in the composition root. `None` in every test
    /// that does not wire one, same contract as `perm_compiler`.
    pub catalog_assignment_compiler:
        Option<Arc<dyn egret_catalog_registry::CatalogAssignmentCompiler>>,
}

impl FromRef<EngineMgmtState> for SessionService {
    fn from_ref(s: &EngineMgmtState) -> Self {
        s.sessions.clone()
    }
}
