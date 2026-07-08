//! `egret` binary entrypoint: parse the CLI, load config, dispatch the command.

mod cli;

use anyhow::Context;
use chrono::Duration;
use clap::Parser;
use cli::{Cli, Command, WorkspaceAction};
use egret_auth::groups::GroupRepo;
use egret_auth::password::hash_password;
use egret_auth::ratelimit::RateLimiter;
use egret_auth::routes::AuthState;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_config::Config;
use egret_metastore::Db;
use egret_server::{build_router, AppState};
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::routes::WorkspaceState;
use egret_workspace::workspaces::WorkspaceRepo;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // kube-rs (engine provisioning) builds a rustls client for the k8s API. The
    // workspace tree enables BOTH rustls crypto providers — `ring` (via sqlx) and
    // `aws-lc-rs` (via lakekeeper's jsonwebtoken) — so rustls cannot auto-select
    // one and panics the moment a TLS client is built. Install a process-wide
    // default up front so every rustls consumer (kube included) has a provider.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();
    let config = Config::load().context("loading configuration")?;
    config.validate().map_err(|e| anyhow::anyhow!(e))?;
    init_tracing(&config);

    match cli.command {
        Command::Serve => serve(config).await,
        Command::Migrate => migrate(config).await,
        Command::Init { force } => init(config, force).await,
        Command::Workspace { action } => workspace_cmd(config, action).await,
    }
}

fn init_tracing(config: &Config) {
    let filter = EnvFilter::try_new(&config.log.filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if config.log.json {
        builder.json().init();
    } else {
        builder.init();
    }
}

async fn migrate(config: Config) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations")?;
    tracing::info!("migrations applied");
    Ok(())
}

/// Bootstrap the first platform admin (local provider). Idempotent: refuses if a
/// platform_admin already exists (unless `force` is set). Prints a generated password when none is supplied.
async fn init(config: Config, force: bool) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations")?;

    if !force {
        let existing: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM platform_role_assignments pra \
             JOIN roles r ON r.id = pra.role_id WHERE r.scope='platform' AND r.name='platform_admin'",
        )
        .fetch_one(&db.pool)
        .await?;
        if existing > 0 {
            tracing::warn!("a platform_admin already exists; use --force to add another");
            return Ok(());
        }
    }

    let email = std::env::var("EGRET_ADMIN_EMAIL").unwrap_or_else(|_| "admin@egret.local".into());
    let password = std::env::var("EGRET_ADMIN_PASSWORD")
        .unwrap_or_else(|_| egret_auth::token::generate_token());
    let hash = hash_password(&password)?;

    let users = UserRepo::new(db.clone());
    let user_id = users.insert_local(&email, "Administrator", &hash).await?;

    sqlx::query(
        "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
         SELECT 'user', $1, id FROM roles WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(user_id.0)
    .execute(&db.pool)
    .await?;

    tracing::info!(%email, "bootstrap admin created");
    println!("Bootstrap admin: {email}");
    println!("Password: {password}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Catalog token management
// ---------------------------------------------------------------------------

/// Helper: connect to the DB (runs migrations so tokens table is present).
async fn connect_db(config: &Config) -> anyhow::Result<Db> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations")?;
    Ok(db)
}

// ---------------------------------------------------------------------------
// Workspace membership management
// ---------------------------------------------------------------------------

async fn workspace_cmd(config: Config, action: WorkspaceAction) -> anyhow::Result<()> {
    let db = connect_db(&config).await?;
    match action {
        WorkspaceAction::AddGroup {
            workspace,
            group,
            role,
        } => {
            let admin_email =
                std::env::var("EGRET_ADMIN_EMAIL").unwrap_or_else(|_| "admin@egret.local".into());
            egret_bin::grant_group_to_workspace(&db, &workspace, &group, &role, &admin_email)
                .await?;
            println!("Granted group '{group}' role '{role}' in workspace '{workspace}'");
            Ok(())
        }
    }
}

/// Engine-connection resolver wired into `KernelState` (T4).
///
/// Resolves a `database_id` to the coordinates a kernel needs to open a direct
/// Daft connection. Backed by the same `DatabaseRepo` registry the query runner
/// uses; the engine label matches `EngineOption.engine` via `Engine::as_str`.
struct EngineConnResolverImpl {
    databases: egret_database::DatabaseRepo,
}

#[async_trait::async_trait]
impl egret_kernel::engine_conn::EngineConnResolver for EngineConnResolverImpl {
    async fn resolve(
        &self,
        database_id: uuid::Uuid,
    ) -> Result<egret_kernel::engine_conn::EngineConn, egret_kernel::engine_conn::EngineConnError>
    {
        use egret_kernel::engine_conn::{EngineConn, EngineConnError};
        let db = self
            .databases
            .get(database_id)
            .await
            .map_err(|e| EngineConnError::NotFound(e.to_string()))?;
        Ok(EngineConn {
            kind: db.engine.as_str().to_string(),
            host: db.host,
            port: db.port,
        })
    }
}

/// egret's authN + authZ guard for the embedded catalog (EB-3). Validates the
/// egret-issued JWT (authN); the JWT's `warehouse`+`roles` claims + the RBAC
/// store drive authZ. lakekeeper itself runs on `AllowAllAuthorizer` — egret owns
/// the decision here.
struct EgretCatalogGuard {
    keys: egret_machine_idp::PgSigningKeyStore,
    catalog: egret_catalog::CatalogStore,
}

#[async_trait::async_trait]
impl egret_catalog_engine::CatalogGuard for EgretCatalogGuard {
    async fn check(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        bearer: Option<&str>,
    ) -> Result<(), (axum::http::StatusCode, String)> {
        use axum::http::StatusCode;
        let forbid = |m: String| Err((StatusCode::FORBIDDEN, m));
        // authN: a valid, unexpired egret JWT is required on every catalog request.
        let token = bearer.ok_or((StatusCode::UNAUTHORIZED, "missing bearer token".into()))?;
        use egret_machine_idp::SigningKeyStore as _;
        let jwk = self
            .keys
            .active()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("idp key: {e}")))?
            .public_jwk;
        let claims = egret_machine_idp::verify_with_jwk(token, &jwk)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid token".to_string()))?;
        let exp = claims.get("exp").and_then(|v| v.as_i64()).unwrap_or(0);
        if exp <= chrono::Utc::now().timestamp() {
            return Err((StatusCode::UNAUTHORIZED, "token expired".to_string()));
        }
        let token_warehouse = claims
            .get("warehouse")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let token_role = claims
            .get("roles")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // authZ, discovery (`GET /config?warehouse=`): a client may only discover
        // the warehouse named in its token.
        if path == "/config" {
            if let Some(requested) = query.and_then(warehouse_query_param) {
                if requested != token_warehouse {
                    return forbid(format!(
                        "token is scoped to warehouse '{token_warehouse}', not '{requested}'"
                    ));
                }
            }
            return Ok(());
        }

        // authZ, data-plane (`/{prefix}/namespaces/{ns}/tables/{table}` …): the
        // prefix must be the token's warehouse (isolation), and a grant must cover
        // the requested namespace/table with sufficient privilege — writes need
        // `write`/`manage`; reads accept any privilege. Grant scope is honoured, so
        // a table-level grant only reaches that one table, not the whole namespace.
        let segs: Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if let Some(&prefix) = segs.first() {
            let wh = self
                .catalog
                .get_warehouse_by_name(token_warehouse)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("warehouse: {e}")))?
                .ok_or((
                    StatusCode::FORBIDDEN,
                    format!("unknown warehouse '{token_warehouse}'"),
                ))?;
            if wh.lakekeeper_warehouse_id.as_deref() != Some(prefix) {
                return forbid("token is not scoped to this warehouse".to_string());
            }
            let grants = self
                .catalog
                .grants_for_role(token_warehouse, token_role)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("grants: {e}")))?;
            if grants.is_empty() {
                return forbid(format!(
                    "role '{token_role}' has no grants on warehouse '{token_warehouse}'"
                ));
            }
            let is_write = matches!(method, "POST" | "PUT" | "DELETE" | "PATCH");

            // Resolve the request target: /namespaces/{ns}[/tables|views/{table}].
            let target_ns: Option<Vec<String>> = (segs.get(1) == Some(&"namespaces"))
                .then(|| segs.get(2))
                .flatten()
                .map(|s| catalog_ns_parts(s));
            let target_table: Option<&str> =
                matches!(segs.get(3), Some(&"tables") | Some(&"views"))
                    .then(|| segs.get(4).copied())
                    .flatten();

            let allowed = if target_ns.is_none() {
                // catalog-root op (list namespaces, transactions, …): any grant for
                // reads, a write/manage grant for writes.
                grants.iter().any(|g| {
                    !is_write
                        || g.privilege.eq_ignore_ascii_case("write")
                        || g.privilege.eq_ignore_ascii_case("manage")
                })
            } else {
                grants
                    .iter()
                    .any(|g| catalog_grant_covers(g, target_ns.as_deref(), target_table, is_write))
            };
            if !allowed {
                return forbid(format!(
                    "role '{token_role}' is not granted {} on the requested resource",
                    if is_write { "write" } else { "read" }
                ));
            }
        }
        Ok(())
    }
}

/// Split a namespace path segment into its parts. lakekeeper encodes multipart
/// Iceberg namespaces with the unit separator (`\x1f`), percent-encoded as `%1F`
/// in the URL; single-level namespaces are just the plain name.
fn catalog_ns_parts(seg: &str) -> Vec<String> {
    seg.replace("%1F", "\u{1f}")
        .replace("%1f", "\u{1f}")
        .split('\u{1f}')
        .map(|s| s.to_string())
        .collect()
}

/// Does one grant authorize the requested `(namespace, table)` at the required
/// privilege? `catalog` covers everything; `namespace` covers its own namespace
/// and everything in it; `table` covers only its exact table (plus, for reads,
/// listing the namespace that holds it). Writes require `write`/`manage`.
fn catalog_grant_covers(
    g: &egret_catalog::service::CatalogGrant,
    target_ns: Option<&[String]>,
    target_table: Option<&str>,
    is_write: bool,
) -> bool {
    if is_write
        && !(g.privilege.eq_ignore_ascii_case("write")
            || g.privilege.eq_ignore_ascii_case("manage"))
    {
        return false;
    }
    match g.securable_type.as_str() {
        "catalog" => true,
        "namespace" => {
            matches!((target_ns, g.namespace.as_deref()), (Some(t), Some(gn)) if t == gn)
        }
        "table" => {
            let ns_match =
                matches!((target_ns, g.namespace.as_deref()), (Some(t), Some(gn)) if t == gn);
            if !ns_match {
                return false;
            }
            match target_table {
                // A specific table request must hit this grant's exact table.
                Some(t) => g.table_name.as_deref() == Some(t),
                // Listing/loading the enclosing namespace is a read affordance for
                // a table-grant holder, never a write (e.g. creating a table).
                None => !is_write,
            }
        }
        _ => false,
    }
}

/// Provisions the embedded lakekeeper warehouse for an egret `warehouses` row.
/// Holds the lakekeeper `ApiContext` (built in `serve`).
struct LakekeeperProvisioner {
    engine_state: egret_catalog_engine::ApiContext<egret_catalog_engine::EngineState>,
    system_user: String,
}

#[async_trait::async_trait]
impl egret_controlplane::WarehouseProvisioner for LakekeeperProvisioner {
    async fn provision(
        &self,
        name: &str,
        s3: &egret_controlplane::ProvisionS3,
    ) -> Result<String, String> {
        let cfg = egret_catalog_engine::S3StorageConfig {
            bucket: s3.bucket.clone(),
            region: s3.region.clone(),
            endpoint: s3.endpoint.clone(),
            path_style: s3.path_style,
            access_key: s3.access_key.clone(),
            secret_key: s3.secret_key.clone(),
            key_prefix: s3.key_prefix.clone(),
        };
        egret_catalog_engine::create_lakekeeper_warehouse(
            self.engine_state.clone(),
            &self.system_user,
            name,
            &cfg,
        )
        .await
        .map_err(|e| e.to_string())
    }
}

/// Provisions query engines (Phase 1: Trino) onto Kubernetes for the
/// engine-mgmt seam. Adapts the kube-free `egret_engine_mgmt_api::EngineProvisioningPort`
/// trait onto the `egret-orchestrator` `TrinoProvisioner`. Since E1a both traits'
/// spec/status types are the SAME `egret-engine-mgmt-api` DTOs (aliased under
/// different names on each side), so this is a pure pass-through — no
/// translation between the two. The 4 kube-shaped ops (exec/pod-logs/
/// coordinator-request) are NOT on the Port — the seam calls
/// `egret_orchestrator::KubeClient` directly via `EngineMgmtState.kube`.
struct EgretEngineProvisioningPort {
    inner: std::sync::Arc<egret_orchestrator::TrinoProvisioner>,
    /// The same KubeClient the `TrinoProvisioner` holds, kept here so
    /// `restart` (which bypasses TrinoProvisioner's render/apply path) can
    /// reach it directly.
    kube: std::sync::Arc<dyn egret_orchestrator::KubeClient>,
}

impl EgretEngineProvisioningPort {
    fn new(
        inner: std::sync::Arc<egret_orchestrator::TrinoProvisioner>,
        kube: std::sync::Arc<dyn egret_orchestrator::KubeClient>,
    ) -> Self {
        Self { inner, kube }
    }
}

#[async_trait::async_trait]
impl egret_engine_mgmt_api::EngineProvisioningPort for EgretEngineProvisioningPort {
    async fn provision(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        egret_orchestrator::EngineProvisioner::provision(self.inner.as_ref(), req)
            .await
            .map_err(|e| e.to_string())
    }

    async fn status(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<egret_engine_mgmt_api::EngineStatus, String> {
        // `egret_engine_mgmt_api::EngineStatus` and `egret_orchestrator::EngineStatus`
        // are now the SAME type (both alias `egret_engine_mgmt_api::EngineStatus`), so
        // the orchestrator's status passes straight through — no reverse mapping.
        egret_orchestrator::EngineProvisioner::status(self.inner.as_ref(), namespace, name)
            .await
            .map_err(|e| e.to_string())
    }

    async fn update(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        egret_orchestrator::EngineProvisioner::update(self.inner.as_ref(), req)
            .await
            .map_err(|e| e.to_string())
    }

    async fn delete(&self, namespace: &str, name: &str) -> Result<(), String> {
        egret_orchestrator::EngineProvisioner::delete(self.inner.as_ref(), namespace, name)
            .await
            .map_err(|e| e.to_string())
    }

    async fn restart(&self, namespace: &str, name: &str) -> Result<(), String> {
        // Roll BOTH Deployments directly via KubeClient (bypassing the
        // TrinoProvisioner render/apply path entirely) so this never touches
        // config.
        self.kube
            .restart_deployment(namespace, &format!("{name}-coordinator"))
            .await
            .map_err(|e| e.to_string())?;
        self.kube
            .restart_deployment(namespace, &format!("{name}-worker"))
            .await
            .map_err(|e| e.to_string())
    }
}

/// Reads catalog structure (namespaces/tables/schema) from the embedded
/// lakekeeper for the control plane. Holds the lakekeeper `ApiContext` (like
/// `LakekeeperProvisioner`).
struct LakekeeperBrowser {
    engine_state: egret_catalog_engine::ApiContext<egret_catalog_engine::EngineState>,
    system_user: String,
}

#[async_trait::async_trait]
impl egret_controlplane::WarehouseBrowser for LakekeeperBrowser {
    async fn namespaces(
        &self,
        wh: &str,
    ) -> Result<Vec<egret_controlplane::BrowseNamespace>, String> {
        egret_catalog_engine::list_namespaces(self.engine_state.clone(), &self.system_user, wh)
            .await
            .map(|v| {
                v.into_iter()
                    .map(|n| egret_controlplane::BrowseNamespace {
                        parts: n.parts,
                        id: n.id,
                        protected: n.protected,
                    })
                    .collect()
            })
            .map_err(|e| e.to_string())
    }

    async fn tabulars(
        &self,
        wh: &str,
        ns: &[String],
    ) -> Result<Vec<egret_controlplane::BrowseTabular>, String> {
        egret_catalog_engine::list_tabulars(self.engine_state.clone(), &self.system_user, wh, ns)
            .await
            .map(|v| {
                v.into_iter()
                    .map(|t| egret_controlplane::BrowseTabular {
                        name: t.name,
                        kind: match t.kind {
                            egret_catalog_engine::TabularKind::View => "view",
                            egret_catalog_engine::TabularKind::Table => "table",
                        }
                        .to_string(),
                        id: t.id,
                        protected: t.protected,
                    })
                    .collect()
            })
            .map_err(|e| e.to_string())
    }

    async fn schema(
        &self,
        wh: &str,
        ns: &[String],
        table: &str,
    ) -> Result<egret_controlplane::BrowseTableDetail, String> {
        egret_catalog_engine::load_table_detail(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            ns,
            table,
        )
        .await
        .map(|d| egret_controlplane::BrowseTableDetail {
            location: d.location,
            format_version: d.format_version,
            last_updated_ms: d.last_updated_ms,
            current_snapshot_id: d.current_snapshot_id,
            snapshot_count: d.snapshot_count,
            columns: d
                .columns
                .into_iter()
                .map(|c| egret_controlplane::BrowseColumn {
                    name: c.name,
                    type_: c.r#type,
                    required: c.required,
                })
                .collect(),
            partition_fields: d
                .partition_fields
                .into_iter()
                .map(|p| egret_controlplane::BrowsePartition {
                    name: p.name,
                    source_column: p.source_column,
                    transform: p.transform,
                })
                .collect(),
            sort_fields: d
                .sort_fields
                .into_iter()
                .map(|s| egret_controlplane::BrowseSort {
                    column: s.column,
                    transform: s.transform,
                    direction: s.direction,
                    null_order: s.null_order,
                })
                .collect(),
            properties: d.properties,
            snapshots: d
                .snapshots
                .into_iter()
                .map(|s| egret_controlplane::BrowseSnapshot {
                    snapshot_id: s.snapshot_id,
                    parent_snapshot_id: s.parent_snapshot_id,
                    timestamp_ms: s.timestamp_ms,
                    operation: s.operation,
                    summary: s.summary,
                })
                .collect(),
        })
        .map_err(|e| e.to_string())
    }

    async fn view_columns(
        &self,
        wh: &str,
        ns: &[String],
        view: &str,
    ) -> Result<Vec<egret_controlplane::BrowseColumn>, String> {
        egret_catalog_engine::load_view_detail(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            ns,
            view,
        )
        .await
        .map(|cols| {
            cols.into_iter()
                .map(|c| egret_controlplane::BrowseColumn {
                    name: c.name,
                    type_: c.r#type,
                    required: c.required,
                })
                .collect()
        })
        .map_err(|e| e.to_string())
    }

    async fn deleted_tabulars(
        &self,
        wh: &str,
    ) -> Result<Vec<egret_controlplane::BrowseDeletedTabular>, String> {
        egret_catalog_engine::list_deleted_tabulars(
            self.engine_state.clone(),
            &self.system_user,
            wh,
        )
        .await
        .map(|v| {
            v.into_iter()
                .map(|d| egret_controlplane::BrowseDeletedTabular {
                    id: d.id,
                    name: d.name,
                    namespace: d.namespace,
                    kind: match d.kind {
                        egret_catalog_engine::TabularKindName::View => "view",
                        egret_catalog_engine::TabularKindName::Table => "table",
                    }
                    .to_string(),
                    deleted_at: d.deleted_at,
                    expiration: d.expiration,
                })
                .collect()
        })
        .map_err(|e| e.to_string())
    }

    async fn undrop(&self, wh: &str, kind: &str, id: uuid::Uuid) -> Result<(), String> {
        let k = match kind {
            "view" => egret_catalog_engine::TabularKindName::View,
            _ => egret_catalog_engine::TabularKindName::Table,
        };
        egret_catalog_engine::undrop_tabular(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            k,
            id,
        )
        .await
        .map_err(|e| e.to_string())
    }

    async fn set_protection(
        &self,
        wh: &str,
        target: &str,
        id: uuid::Uuid,
        protected: bool,
    ) -> Result<bool, String> {
        let t = match target {
            "namespace" => egret_catalog_engine::ProtectTarget::Namespace,
            "view" => egret_catalog_engine::ProtectTarget::View,
            _ => egret_catalog_engine::ProtectTarget::Table,
        };
        egret_catalog_engine::set_protection(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            t,
            id,
            protected,
        )
        .await
        .map_err(|e| e.to_string())
    }

    async fn config(&self, wh: &str) -> Result<egret_controlplane::BrowseWarehouseConfig, String> {
        egret_catalog_engine::get_warehouse_config(self.engine_state.clone(), &self.system_user, wh)
            .await
            .map(|c| egret_controlplane::BrowseWarehouseConfig {
                active: c.active,
                delete_profile_seconds: c.delete_profile_seconds,
                num_tables: c.num_tables,
                num_views: c.num_views,
                stats_updated_ms: c.stats_updated_ms,
            })
            .map_err(|e| e.to_string())
    }

    async fn set_active(&self, wh: &str, active: bool) -> Result<(), String> {
        egret_catalog_engine::set_warehouse_active(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            active,
        )
        .await
        .map_err(|e| e.to_string())
    }

    async fn set_delete_profile(&self, wh: &str, seconds: Option<i64>) -> Result<(), String> {
        egret_catalog_engine::set_delete_profile(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            seconds,
        )
        .await
        .map_err(|e| e.to_string())
    }

    async fn set_format_versions(&self, wh: &str, versions: Vec<u8>) -> Result<(), String> {
        egret_catalog_engine::set_format_versions(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            &versions,
        )
        .await
        .map_err(|e| e.to_string())
    }

    async fn rotate_credential(
        &self,
        wh: &str,
        access_key: &str,
        secret_key: &str,
    ) -> Result<(), String> {
        egret_catalog_engine::rotate_storage_credential(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            access_key,
            secret_key,
        )
        .await
        .map_err(|e| e.to_string())
    }

    async fn search(
        &self,
        wh: &str,
        query: &str,
    ) -> Result<Vec<egret_controlplane::BrowseSearchResult>, String> {
        egret_catalog_engine::search_tabulars(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            query,
        )
        .await
        .map(|v| {
            v.into_iter()
                .map(|r| egret_controlplane::BrowseSearchResult {
                    namespace: r.namespace,
                    name: r.name,
                    kind: r.kind.to_string(),
                })
                .collect()
        })
        .map_err(|e| e.to_string())
    }

    async fn tasks(&self, wh: &str) -> Result<Vec<egret_controlplane::BrowseTask>, String> {
        egret_catalog_engine::list_tasks(self.engine_state.clone(), &self.system_user, wh)
            .await
            .map(|v| {
                v.into_iter()
                    .map(|t| egret_controlplane::BrowseTask {
                        queue: t.queue,
                        entity: t.entity,
                        status: t.status,
                    })
                    .collect()
            })
            .map_err(|e| e.to_string())
    }
}

/// Extract the `warehouse` query-param value from a raw query string
/// (e.g. `"warehouse=lakehouse&x=y"` → `Some("lakehouse")`).
fn warehouse_query_param(query: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        kv.split_once('=')
            .filter(|(k, _)| *k == "warehouse")
            .map(|(_, v)| v.replace('+', " "))
    })
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations on boot")?;

    // Warn (do not fail) if any stored PLATFORM-scope role permission is unknown to the
    // catalog — catches drift between the DB and egret_core::rbac. In debug builds, fail
    // fast instead: an unknown key almost always means the catalog and a migration/seed
    // have drifted apart during development, cheaper to catch at `cargo run` time than as
    // a silent dead grant in prod. Only platform-scope grants are checked here:
    // `egret_core::rbac` is the platform vocabulary; workspace-scope roles (ws_*) use a
    // separate permission vocabulary owned by egret-workspace (dataset/chart/notebook/…).
    if let Ok(rows) = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT rp.permission FROM role_permissions rp \
         JOIN roles r ON r.id = rp.role_id WHERE r.scope = 'platform'",
    )
    .fetch_all(&db.pool)
    .await
    {
        let unknown: Vec<&String> = rows
            .iter()
            .filter(|p| !egret_core::rbac::is_known(p))
            .collect();
        #[cfg(debug_assertions)]
        if !unknown.is_empty() {
            panic!(
                "role_permissions references permissions not in egret_core::rbac::PERMISSIONS: {unknown:?}"
            );
        }
        for perm in unknown {
            tracing::warn!(permission = %perm, "role_permissions references a permission not in the catalog");
        }
    }

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        Duration::hours(config.session.absolute_ttl_hours as i64),
        Duration::minutes(config.session.idle_ttl_minutes as i64),
    );
    let rate_limiter = RateLimiter::new(
        config.rate_limit.login_max_attempts,
        config.rate_limit.login_window_secs,
        config.rate_limit.login_lockout_secs,
    );
    let auth = AuthState {
        users: users.clone(),
        sessions: sessions.clone(),
        config: config.clone(),
        rate_limiter,
    };

    // Shared secret store: one `SecretStore` built from `db` + the master key,
    // reused by the per-workspace storage layer and the database-connection
    // secrets. `SecretStore` is cheap + `Clone`.
    let secrets = {
        use base64::Engine as _;
        let crypto = if config.secrets.master_key.is_empty() {
            None
        } else {
            let key = base64::engine::general_purpose::STANDARD
                .decode(config.secrets.master_key.as_bytes())
                .map_err(|e| anyhow::anyhow!("secrets.master_key base64: {e}"))?;
            Some(egret_secrets::Crypto::new(&key).map_err(|e| anyhow::anyhow!(e.to_string()))?)
        };
        egret_secrets::SecretStore::new(db.clone(), crypto)
    };

    // Epic B: the named per-workspace secret layer over the vault. Built once from
    // the shared vault and threaded into the browse + database states so storage
    // and database connections reference named secrets.
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());

    // Externally-reachable catalog base URL, forwarded to the notebook-runtime
    // manifest as the catalog URL (what pyiceberg/Daft hit).
    let catalog_base_url = config
        .catalog
        .public_url
        .clone()
        .unwrap_or_else(|| "http://localhost:8080".into());

    let workspace = WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: MemberRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        users: users.clone(),
        sessions: sessions.clone(),
    };

    let oidc = if config.oidc.enabled {
        use axum_extra::extract::cookie::Key;
        use base64::Engine;
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(config.security.secret_key.as_bytes())
            .map_err(|e| anyhow::anyhow!("security.secret_key base64: {e}"))?;
        let service = egret_auth::oidc::OidcService::discover(&config.oidc)
            .await
            .map_err(|e| anyhow::anyhow!("oidc discovery: {e}"))?;
        Some(egret_auth::oidc_routes::OidcRoutesState {
            oidc: service,
            users: users.clone(),
            groups: GroupRepo::new(db.clone()),
            sessions: SessionService::new(
                db.clone(),
                Duration::hours(config.session.absolute_ttl_hours as i64),
                Duration::minutes(config.session.idle_ttl_minutes as i64),
            ),
            config: config.clone(),
            cookie_key: Key::from(&key_bytes),
            secrets: secrets.clone(),
            refresh_repo: egret_auth::oidc_refresh::OidcRefreshRepo::new(db.clone()),
        })
    } else {
        None
    };

    let ranger = if config.ranger.enabled {
        let store = egret_authz::AuthzStore::new(db.clone());
        egret_authz::seed_trino(&store)
            .await
            .map_err(anyhow::Error::from)?;
        Some(egret_ranger::RangerState {
            store,
            auth_token: config.ranger.auth_token.clone().into(),
        })
    } else {
        None
    };

    // `secrets` (the SecretStore) is shared: the per-workspace storage layer and
    // the database block reuse the same instance for their secrets.
    let databases =
        egret_database::DatabaseRepo::new(db.clone(), secrets.clone(), workspace_secrets.clone());

    let database = Some(egret_database::DatabaseState {
        repo: databases.clone(),
        authz: Authorizer::new(db.clone()),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        workspace_secrets: workspace_secrets.clone(),
    });

    let engine = {
        let storage_cfg = egret_storage::StorageConfig {
            endpoint: config.storage.endpoint.clone(),
            region: config.storage.region.clone(),
            bucket: config.storage.bucket.clone(),
            access_key: config.storage.access_key.clone(),
            secret_key: config.storage.secret_key.clone(),
            path_style: config.storage.path_style,
        };
        let storage: std::sync::Arc<dyn egret_storage::ObjectStore> =
            std::sync::Arc::new(egret_storage::S3Store::new(&storage_cfg).await?);
        let store = egret_engine::QueryStore::new(db.clone());
        let limits = egret_engine::model::Limits {
            max_rows: config.engine.max_rows,
            timeout: std::time::Duration::from_secs(config.engine.query_timeout_secs),
        };
        let bucket_uri = format!("s3://{}", config.storage.bucket);
        let jobs = egret_engine::JobManager::new(
            store.clone(),
            databases.clone(),
            std::sync::Arc::new(egret_engine::connector::DefaultConnectorFactory),
            storage.clone(),
            bucket_uri.clone(),
            config.engine.results_prefix.clone(),
            limits.clone(),
            config.engine.preview_rows,
        );
        Some(egret_engine::EngineState {
            store,
            jobs,
            databases: databases.clone(),
            storage,
            bucket_uri: bucket_uri.into(),
            results_prefix: config.engine.results_prefix.clone().into(),
            limits,
            sessions: sessions.clone(),
            workspaces: WorkspaceRepo::new(db.clone()),
            authz: Authorizer::new(db.clone()),
            users: UserRepo::new(db.clone()),
        })
    };

    let bi = Some(egret_bi::BiState {
        saved: egret_bi::SavedQueryRepo::new(db.clone()),
        schema: egret_bi::SchemaService::new(),
        databases: databases.clone(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        charts: egret_bi::ChartRepo::new(db.clone()),
        datasets: egret_bi::DatasetRepo::new(db.clone()),
        dashboards: egret_bi::DashboardRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db.clone()),
    });

    // F2-P1 T6: one shared engine-token broker (Keycloak refresh-grant) mints the
    // short-lived per-user engine JWT carried in the notebook-runtime manifest.
    // When OIDC is disabled the deps are empty strings, so this still constructs
    // and only fails at mint-time. Task 7: reuse the SAME Arc for KernelState.
    let engine_minter: std::sync::Arc<dyn egret_notebook::engine_token::EngineTokenMinter> = {
        use egret_bin::engine_broker::{EngineTokenBroker, KeycloakRefresher};
        std::sync::Arc::new(EngineTokenBroker::new(
            secrets.clone(),
            egret_auth::oidc_refresh::OidcRefreshRepo::new(db.clone()),
            KeycloakRefresher {
                http: reqwest::Client::new(),
                token_url: format!(
                    "{}/protocol/openid-connect/token",
                    config.oidc.issuer_url.trim_end_matches('/')
                ),
                client_id: config.oidc.client_id.clone(),
                client_secret: config.oidc.client_secret.clone(),
            },
        ))
    };

    let notebook_state = egret_notebook::NotebookState {
        store: egret_notebook::NotebookStore::new(db.clone()),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        users: UserRepo::new(db.clone()),
        catalog_base_url: catalog_base_url.clone(),
        engine_minter: engine_minter.clone(),
    };
    let notebook = Some(notebook_state.clone());

    // ── Kernel lifecycle wiring (D1b T8) ────────────────────────────────────────
    // Startup sweep: subprocess kernels from a prior run are gone — clear stale rows.
    sqlx::query("DELETE FROM kernels")
        .execute(&db.pool)
        .await
        .ok();

    let kernel_registry = egret_kernel::KernelRegistry::new(db.clone());
    let kernel_launcher: std::sync::Arc<dyn egret_kernel::KernelLauncher> =
        std::sync::Arc::new(egret_kernel::SubprocessLauncher::default());
    let kernel_idle = std::time::Duration::from_secs(30 * 60); // 30 min idle timeout

    let kernel_manager = std::sync::Arc::new(egret_kernel::KernelManager::new(
        kernel_registry.clone(),
        kernel_launcher.clone(),
        kernel_idle,
    ));

    // Spawn the idle reaper (sweeps every 60 s; cancelled via the token on SIGINT).
    let reaper_cancel = tokio_util::sync::CancellationToken::new();
    egret_kernel::spawn_reaper(
        kernel_registry,
        kernel_launcher,
        kernel_idle,
        std::time::Duration::from_secs(60),
        reaper_cancel.clone(),
    );
    {
        let reaper_cancel = reaper_cancel.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("SIGINT received, stopping kernel reaper");
            reaper_cancel.cancel();
        });
    }

    // api_url: the server's own externally-reachable base URL (for EGRET_API_URL
    // forwarded to kernel processes as a callback base). Defaults to
    // http://127.0.0.1:{port} from the configured listen address when catalog
    // public_url is not set.
    let api_url = config
        .catalog
        .public_url
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", config.server.port));

    let kernel = Some(egret_kernel::KernelState {
        manager: kernel_manager,
        notebook: notebook_state,
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        api_url,
        notebook_kernels: egret_kernel::NotebookKernelStore::new(db.clone()),
        proxy: std::sync::Arc::new(egret_kernel::BackendWsProxy),
        engine_conn: std::sync::Arc::new(EngineConnResolverImpl {
            databases: databases.clone(),
        }),
        // Task 7: reuse the SAME broker Arc as NotebookState — one minter.
        engine_minter: engine_minter.clone(),
        active: egret_kernel::recorder::ActiveExecutions::default(),
    });
    // ── end kernel wiring ───────────────────────────────────────────────────────

    // Embedded catalog (EB-1/EB-3): mount lakekeeper in-process at /catalog/v1
    // (fronted by egret's CatalogGuard), and build the warehouse provisioner —
    // both need the lakekeeper ApiContext built here.
    let mut catalog_provisioner: Option<
        std::sync::Arc<dyn egret_controlplane::WarehouseProvisioner>,
    > = None;
    let mut catalog_browser: Option<std::sync::Arc<dyn egret_controlplane::WarehouseBrowser>> =
        None;
    let catalog_engine: Option<axum::Router> = if config.catalog.enabled {
        let dsn = config.database.url.as_str();
        egret_catalog_engine::run_lakekeeper_migrations(dsn)
            .await
            .context("lakekeeper migrations")?;
        let (engine_state, gc_registry) =
            egret_catalog_engine::build_state(dsn, &config.catalog.system_user)
                .await
                .context("building catalog engine state")?;
        egret_catalog_engine::ensure_bootstrapped(
            engine_state.clone(),
            &config.catalog.system_user,
        )
        .await
        .context("bootstrapping catalog engine")?;
        let gc_cancel = egret_catalog_engine::CancellationToken::new();
        egret_catalog_engine::spawn_gc_workers(gc_registry, gc_cancel);
        catalog_provisioner = Some(std::sync::Arc::new(LakekeeperProvisioner {
            engine_state: engine_state.clone(),
            system_user: config.catalog.system_user.clone(),
        }));
        catalog_browser = Some(std::sync::Arc::new(LakekeeperBrowser {
            engine_state: engine_state.clone(),
            system_user: config.catalog.system_user.clone(),
        }));
        let guard: std::sync::Arc<dyn egret_catalog_engine::CatalogGuard> =
            std::sync::Arc::new(EgretCatalogGuard {
                keys: egret_machine_idp::PgSigningKeyStore::new(db.clone()),
                catalog: egret_catalog::CatalogStore::new(db.clone()),
            });
        Some(egret_catalog_engine::mount_catalog_engine(
            engine_state,
            Some(guard),
        ))
    } else {
        None
    };

    // Engine provisioning: build a kube client best-effort (in-cluster first, then
    // the local kubeconfig for dev). If neither is available egret still boots —
    // engine routes then report "kubernetes not configured". Both the kube-free
    // Port (provision/status/update/delete/restart) and the raw `KubeClient`
    // (exec/pod-logs/coordinator-request, served straight off `EngineMgmtState`)
    // are derived from the SAME kube client.
    let kube: Option<std::sync::Arc<dyn egret_orchestrator::KubeClient>> = {
        let kube = match egret_orchestrator::LiveKube::in_cluster().await {
            Ok(k) => Some(k),
            Err(_) => egret_orchestrator::LiveKube::from_kubeconfig().await.ok(),
        };
        match kube {
            Some(k) => Some(std::sync::Arc::new(k)),
            None => {
                tracing::warn!("kubernetes unavailable; engine provisioning disabled");
                None
            }
        }
    };
    let engine_provisioner: Option<std::sync::Arc<dyn egret_engine_mgmt_api::EngineProvisioningPort>> =
        kube.clone().map(|kube| {
            let trino = egret_orchestrator::TrinoProvisioner::new(kube.clone());
            std::sync::Arc::new(EgretEngineProvisioningPort::new(
                std::sync::Arc::new(trino),
                kube,
            )) as std::sync::Arc<dyn egret_engine_mgmt_api::EngineProvisioningPort>
        });

    // In-cluster URL Trino pods use to reach egret's REST catalog + token
    // endpoint; shared by the engine-mgmt seam (provisioning) and control-plane
    // (policies' `trino_config` route renders the same base URL).
    let engine_egret_url = config
        .catalog
        .public_url
        .clone()
        .unwrap_or_else(|| "http://egret:8080".to_string());

    // Control plane: global platform-admin Storage + Secrets + embedded-catalog
    // management. The warehouse provisioner (above) creates lakekeeper warehouses.
    // `engine_mgmt`/`engine_egret_url` are cross-seam, read-only: the Policies
    // module surfaces which engines/catalogs bind to a Ranger service.
    let control_plane = Some(egret_controlplane::CpState {
        storage: egret_catalog::CatalogStore::new(db.clone()),
        authz: egret_authz::AuthzStore::new(db.clone()),
        users: egret_auth::users::UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone()),
        secrets: workspace_secrets.clone(),
        sessions: sessions.clone(),
        provisioner: catalog_provisioner,
        browser: catalog_browser,
        engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
        engine_egret_url: engine_egret_url.clone(),
    });

    // Engine-mgmt seam: owns the engine domain (HTTP router + provisioning).
    let engine_mgmt = Some(egret_engine_mgmt::EngineMgmtState {
        engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
        storage: egret_catalog::CatalogStore::new(db.clone()),
        authz: egret_authz::AuthzStore::new(db.clone()),
        secrets: workspace_secrets.clone(),
        sessions: sessions.clone(),
        engine_provisioner,
        kube,
        engine_egret_url,
        // Default JWKS the JWT authenticator validates engine tokens against.
        // Derived from the OIDC realm (Keycloak `.../realms/<realm>` issuer):
        // JWKS lives at `.../protocol/openid-connect/certs`. Empty when OIDC is
        // unconfigured — the admin then sets `jwksUri` per engine. Not hard-failed.
        engine_jwks_uri: if config.oidc.enabled && !config.oidc.issuer_url.is_empty() {
            format!(
                "{}/protocol/openid-connect/certs",
                config.oidc.issuer_url.trim_end_matches('/')
            )
        } else {
            String::new()
        },
    });

    // egret mini machine-IdP: mints Catalog Client tokens (/oauth/token + JWKS).
    // EB-3: the real ClientVerifier is the Catalog Client registry (CatalogStore).
    // `issuer` is egret's public root URL (the JWT `iss` + OIDC issuer lakekeeper
    // will trust in EB-3's Authorizer integration).
    let idp = Some(egret_machine_idp::IdpState {
        keys: std::sync::Arc::new(egret_machine_idp::PgSigningKeyStore::new(db.clone())),
        issuer: config
            .catalog
            .public_url
            .clone()
            .unwrap_or_else(|| "http://localhost:8080".to_string()),
        clients: std::sync::Arc::new(egret_catalog::CatalogStore::new(db.clone())),
    });

    let app = build_router(AppState {
        db: Some(db),
        auth: Some(auth),
        workspace: Some(workspace),
        oidc,
        ranger,
        database,
        engine,
        bi,
        notebook,
        kernel,
        control_plane,
        engine_mgmt,
        catalog_engine,
        idp,
    });
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "egret listening");
    axum::serve(listener, app).await.context("server error")?;
    Ok(())
}

#[cfg(test)]
mod catalog_authz_tests {
    use super::{catalog_grant_covers, catalog_ns_parts};
    use egret_catalog::service::CatalogGrant;
    use uuid::Uuid;

    fn grant(sec: &str, ns: Option<&[&str]>, table: Option<&str>, privilege: &str) -> CatalogGrant {
        CatalogGrant {
            id: Uuid::nil(),
            catalog_role_id: Uuid::nil(),
            securable_type: sec.to_string(),
            namespace: ns.map(|p| p.iter().map(|s| s.to_string()).collect()),
            table_name: table.map(|s| s.to_string()),
            privilege: privilege.to_string(),
        }
    }

    #[test]
    fn table_grant_reaches_only_its_table() {
        let g = grant("table", Some(&["demo_ns"]), Some("events"), "read");
        let ns = vec!["demo_ns".to_string()];
        assert!(catalog_grant_covers(&g, Some(&ns), Some("events"), false));
        assert!(!catalog_grant_covers(
            &g,
            Some(&ns),
            Some("customers"),
            false
        ));
        let other = vec!["other".to_string()];
        assert!(!catalog_grant_covers(
            &g,
            Some(&other),
            Some("events"),
            false
        ));
    }

    #[test]
    fn read_grant_denies_writes() {
        let g = grant("table", Some(&["demo_ns"]), Some("events"), "read");
        let ns = vec!["demo_ns".to_string()];
        assert!(!catalog_grant_covers(&g, Some(&ns), Some("events"), true));
    }

    #[test]
    fn namespace_grant_covers_its_tables_only() {
        let g = grant("namespace", Some(&["demo_ns"]), None, "read");
        let ns = vec!["demo_ns".to_string()];
        assert!(catalog_grant_covers(&g, Some(&ns), Some("anything"), false));
        let other = vec!["other".to_string()];
        assert!(!catalog_grant_covers(&g, Some(&other), Some("x"), false));
    }

    #[test]
    fn catalog_grant_covers_everything() {
        let g = grant("catalog", None, None, "write");
        let ns = vec!["demo_ns".to_string()];
        assert!(catalog_grant_covers(&g, Some(&ns), Some("events"), true));
    }

    #[test]
    fn ns_parts_handles_multipart() {
        assert_eq!(catalog_ns_parts("demo_ns"), vec!["demo_ns".to_string()]);
        assert_eq!(
            catalog_ns_parts("a%1Fb"),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
