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

/// Provisions query engines (Phase 1: Trino) onto Kubernetes for the control
/// plane. Adapts the kube-free `egret_controlplane::EngineProvisioner`
/// trait onto the `egret-orchestrator` `TrinoProvisioner`, mapping the
/// `EngineProvisionReq` (catalogs + config) into an `EngineSpec` and the k8s
/// status back out.
struct EgretEngineProvisioner {
    inner: std::sync::Arc<egret_orchestrator::TrinoProvisioner>,
    /// The same KubeClient the `TrinoProvisioner` holds, kept here so the
    /// kube-free `coordinator_request` seam can reach it directly.
    kube: std::sync::Arc<dyn egret_orchestrator::KubeClient>,
}

impl EgretEngineProvisioner {
    fn new(
        inner: std::sync::Arc<egret_orchestrator::TrinoProvisioner>,
        kube: std::sync::Arc<dyn egret_orchestrator::KubeClient>,
    ) -> Self {
        Self { inner, kube }
    }

    /// Map a single control-plane `ProbeReq` onto the orchestrator `Probe`
    /// (field-for-field; both are pure option carriers).
    fn probe_from(p: &egret_controlplane::ProbeReq) -> egret_orchestrator::Probe {
        egret_orchestrator::Probe {
            initial_delay_seconds: p.initial_delay_seconds,
            period_seconds: p.period_seconds,
            timeout_seconds: p.timeout_seconds,
            failure_threshold: p.failure_threshold,
            success_threshold: p.success_threshold,
        }
    }

    /// Map a per-role `RoleConfigReq` onto the orchestrator `RoleConfig`,
    /// reused for the coordinator and the worker's base config.
    fn role_from(r: &egret_controlplane::RoleConfigReq) -> egret_orchestrator::RoleConfig {
        egret_orchestrator::RoleConfig {
            replicas: r.replicas,
            resources: egret_orchestrator::RoleResources {
                cpu_request: r.resources.cpu_request.clone(),
                cpu_limit: r.resources.cpu_limit.clone(),
                mem_request: r.resources.mem_request.clone(),
                mem_limit: r.resources.mem_limit.clone(),
            },
            heap: egret_orchestrator::HeapConfig {
                max_size: r.heap.max_size.clone(),
                min_size: r.heap.min_size.clone(),
                max_percent: r.heap.max_percent,
                initial_percent: r.heap.initial_percent,
            },
            query_max_memory_per_node: r.query_max_memory_per_node.clone(),
            additional_jvm_config: r.additional_jvm_config.clone(),
            heap_headroom_per_node: r.heap_headroom_per_node.clone(),
            probes: egret_orchestrator::Probes {
                liveness: r.probes.liveness.as_ref().map(Self::probe_from),
                readiness: r.probes.readiness.as_ref().map(Self::probe_from),
                startup: r.probes.startup.as_ref().map(Self::probe_from),
            },
            node_selector: r.node_selector.clone(),
            tolerations: r.tolerations.iter().map(Self::toleration_from).collect(),
            topology_spread: r
                .topology_spread
                .iter()
                .map(Self::topology_spread_from)
                .collect(),
            affinity: r.affinity.as_ref().map(Self::affinity_from),
            termination_grace_seconds: r.termination_grace_seconds,
            additional_exposed_ports: r
                .additional_exposed_ports
                .iter()
                .map(Self::container_port_from)
                .collect(),
        }
    }

    /// Map a control-plane `TolerationReq` onto the orchestrator `Toleration`.
    fn toleration_from(t: &egret_controlplane::TolerationReq) -> egret_orchestrator::Toleration {
        egret_orchestrator::Toleration {
            key: t.key.clone(),
            operator: t.operator.clone(),
            value: t.value.clone(),
            effect: t.effect.clone(),
            toleration_seconds: t.toleration_seconds,
        }
    }

    /// Map a `TopologySpreadConstraintReq` onto the orchestrator type.
    fn topology_spread_from(
        t: &egret_controlplane::TopologySpreadConstraintReq,
    ) -> egret_orchestrator::TopologySpreadConstraint {
        egret_orchestrator::TopologySpreadConstraint {
            max_skew: t.max_skew,
            topology_key: t.topology_key.clone(),
            when_unsatisfiable: t.when_unsatisfiable.clone(),
            match_labels: t.match_labels.clone(),
        }
    }

    /// Map a `NodeSelectorRequirementReq` onto the orchestrator type.
    fn node_selector_requirement_from(
        n: &egret_controlplane::NodeSelectorRequirementReq,
    ) -> egret_orchestrator::NodeSelectorRequirement {
        egret_orchestrator::NodeSelectorRequirement {
            key: n.key.clone(),
            operator: n.operator.clone(),
            values: n.values.clone(),
        }
    }

    /// Map an `AffinityReq` onto the orchestrator `Affinity` (structured node
    /// match + anti-affinity key + verbatim raw override).
    fn affinity_from(a: &egret_controlplane::AffinityReq) -> egret_orchestrator::Affinity {
        egret_orchestrator::Affinity {
            node_match: a
                .node_match
                .iter()
                .map(Self::node_selector_requirement_from)
                .collect(),
            pod_anti_affinity_topology_key: a.pod_anti_affinity_topology_key.clone(),
            raw_override: a.raw_override.clone(),
        }
    }

    /// Map a `ContainerPortReq` onto the orchestrator `ContainerPort`.
    fn container_port_from(
        p: &egret_controlplane::ContainerPortReq,
    ) -> egret_orchestrator::ContainerPort {
        egret_orchestrator::ContainerPort {
            name: p.name.clone(),
            container_port: p.container_port,
        }
    }

    /// Map a `SecurityContextReq` onto the orchestrator `SecurityContext`.
    fn security_context_from(
        s: &egret_controlplane::SecurityContextReq,
    ) -> egret_orchestrator::SecurityContext {
        egret_orchestrator::SecurityContext {
            run_as_user: s.run_as_user,
            run_as_group: s.run_as_group,
            fs_group: s.fs_group,
            run_as_non_root: s.run_as_non_root,
            raw_override: s.raw_override.clone(),
        }
    }

    /// Map a `VolumeMountReq` onto the orchestrator `VolumeMount`.
    fn volume_mount_from(
        v: &egret_controlplane::VolumeMountReq,
    ) -> egret_orchestrator::VolumeMount {
        egret_orchestrator::VolumeMount {
            name: v.name.clone(),
            mount_path: v.mount_path.clone(),
            sub_path: v.sub_path.clone(),
        }
    }

    /// Map a `MiniContainerReq` onto the orchestrator `MiniContainer`
    /// (structured-common fields + verbatim raw override).
    fn mini_container_from(
        c: &egret_controlplane::MiniContainerReq,
    ) -> egret_orchestrator::MiniContainer {
        egret_orchestrator::MiniContainer {
            name: c.name.clone(),
            image: c.image.clone(),
            command: c.command.clone(),
            args: c.args.clone(),
            env: c.env.clone(),
            volume_mounts: c
                .volume_mounts
                .iter()
                .map(Self::volume_mount_from)
                .collect(),
            raw_override: c.raw_override.clone(),
        }
    }

    /// Map an `EnvFromSourceReq` onto the orchestrator `EnvFromSource`.
    fn env_from_source_from(
        e: &egret_controlplane::EnvFromSourceReq,
    ) -> egret_orchestrator::EnvFromSource {
        egret_orchestrator::EnvFromSource {
            config_map_ref: e.config_map_ref.clone(),
            secret_ref: e.secret_ref.clone(),
        }
    }

    /// Map an `ObjectMountReq` onto the orchestrator `ObjectMount`.
    fn object_mount_from(
        o: &egret_controlplane::ObjectMountReq,
    ) -> egret_orchestrator::ObjectMount {
        egret_orchestrator::ObjectMount {
            name: o.name.clone(),
            mount_path: o.mount_path.clone(),
            sub_path: o.sub_path.clone(),
            default_mode: o.default_mode,
        }
    }

    /// Map a `DeploymentStrategyReq` onto the orchestrator `DeploymentStrategy`.
    fn deployment_strategy_from(
        d: &egret_controlplane::DeploymentStrategyReq,
    ) -> egret_orchestrator::DeploymentStrategy {
        egret_orchestrator::DeploymentStrategy {
            kind: d.kind.clone(),
            max_surge: d.max_surge.clone(),
            max_unavailable: d.max_unavailable.clone(),
        }
    }

    /// Map a control-plane `ResourceGroupReq` onto the orchestrator
    /// `ResourceGroup`, recursing into sub-groups.
    fn rg_from(g: &egret_controlplane::ResourceGroupReq) -> egret_orchestrator::ResourceGroup {
        egret_orchestrator::ResourceGroup {
            name: g.name.clone(),
            soft_memory_limit: g.soft_memory_limit.clone(),
            hard_concurrency_limit: g.hard_concurrency_limit,
            max_queued: g.max_queued,
            scheduling_policy: g.scheduling_policy.clone(),
            sub_groups: g.sub_groups.iter().map(Self::rg_from).collect(),
        }
    }

    /// Map an `AutoscalingReq` onto the orchestrator `Autoscaling`.
    fn autoscaling_from(a: &egret_controlplane::AutoscalingReq) -> egret_orchestrator::Autoscaling {
        egret_orchestrator::Autoscaling {
            mode: a.mode.clone(),
            hpa: a.hpa.as_ref().map(Self::hpa_config_from),
            keda: a.keda.as_ref().map(Self::keda_config_from),
        }
    }

    /// Map an `HpaConfigReq` onto the orchestrator `HpaConfig`.
    fn hpa_config_from(h: &egret_controlplane::HpaConfigReq) -> egret_orchestrator::HpaConfig {
        egret_orchestrator::HpaConfig {
            max_replicas: h.max_replicas,
            target_cpu_percent: h.target_cpu_percent,
            target_memory_percent: h.target_memory_percent,
            behavior_raw: h.behavior_raw.clone(),
        }
    }

    /// Map a `KedaConfigReq` onto the orchestrator `KedaConfig`.
    fn keda_config_from(k: &egret_controlplane::KedaConfigReq) -> egret_orchestrator::KedaConfig {
        egret_orchestrator::KedaConfig {
            min_replica_count: k.min_replica_count,
            max_replica_count: k.max_replica_count,
            polling_interval: k.polling_interval,
            cooldown_period: k.cooldown_period,
            triggers: k.triggers.iter().map(Self::keda_trigger_from).collect(),
            fallback_raw: k.fallback_raw.clone(),
        }
    }

    /// Map a `KedaTriggerReq` onto the orchestrator `KedaTrigger`.
    fn keda_trigger_from(
        t: &egret_controlplane::KedaTriggerReq,
    ) -> egret_orchestrator::KedaTrigger {
        egret_orchestrator::KedaTrigger {
            kind: t.kind.clone(),
            cron: t.cron.as_ref().map(Self::cron_trigger_from),
            prometheus: t.prometheus.as_ref().map(Self::prometheus_trigger_from),
            raw_override: t.raw_override.clone(),
        }
    }

    /// Map a `CronTriggerReq` onto the orchestrator `CronTrigger`.
    fn cron_trigger_from(
        c: &egret_controlplane::CronTriggerReq,
    ) -> egret_orchestrator::CronTrigger {
        egret_orchestrator::CronTrigger {
            start: c.start.clone(),
            end: c.end.clone(),
            desired_replicas: c.desired_replicas,
            timezone: c.timezone.clone(),
        }
    }

    /// Map a `PrometheusTriggerReq` onto the orchestrator `PrometheusTrigger`.
    fn prometheus_trigger_from(
        p: &egret_controlplane::PrometheusTriggerReq,
    ) -> egret_orchestrator::PrometheusTrigger {
        egret_orchestrator::PrometheusTrigger {
            server_address: p.server_address.clone(),
            metric_name: p.metric_name.clone(),
            query: p.query.clone(),
            threshold: p.threshold.clone(),
        }
    }

    fn spec_from(req: &egret_controlplane::EngineProvisionReq) -> egret_orchestrator::EngineSpec {
        let cfg = &req.config;
        egret_orchestrator::EngineSpec {
            name: req.name.clone(),
            namespace: req.namespace.clone(),
            image_version: req.image_version.clone(),
            config: egret_orchestrator::TrinoConfig {
                query_max_memory: cfg.query_max_memory.clone(),
                log_level: cfg.log_level.clone(),
                config_properties: cfg.config_properties.clone(),
                env: cfg.env.clone(),
                coordinator: Self::role_from(&cfg.coordinator),
                worker: egret_orchestrator::WorkerRoleConfig {
                    base: Self::role_from(&cfg.worker.base),
                    graceful_shutdown: cfg.worker.graceful_shutdown.as_ref().map(|g| {
                        egret_orchestrator::GracefulShutdown {
                            enabled: g.enabled,
                            grace_period_seconds: g.grace_period_seconds,
                        }
                    }),
                },
                additional_node_properties: cfg.additional_node_properties.clone(),
                additional_log_properties: cfg.additional_log_properties.clone(),
                event_listener: cfg.event_listener.clone(),
                resource_groups: cfg.resource_groups.as_ref().map(|rg| {
                    egret_orchestrator::ResourceGroups {
                        root_groups: rg.root_groups.iter().map(Self::rg_from).collect(),
                        selectors: rg
                            .selectors
                            .iter()
                            .map(|s| egret_orchestrator::ResourceSelector {
                                user: s.user.clone(),
                                source: s.source.clone(),
                                query_type: s.query_type.clone(),
                                group: s.group.clone(),
                            })
                            .collect(),
                    }
                }),
                session_properties: cfg
                    .session_properties
                    .iter()
                    .map(|r| egret_orchestrator::SessionRule {
                        match_user: r.match_user.clone(),
                        match_source: r.match_source.clone(),
                        match_client_tags: r.match_client_tags.clone(),
                        properties: r.properties.clone(),
                    })
                    .collect(),
                fault_tolerant: cfg.fault_tolerant.as_ref().map(|f| {
                    egret_orchestrator::FaultTolerant {
                        retry_policy: f.retry_policy.clone(),
                        s3_base_dir: f.s3_base_dir.clone(),
                        s3_endpoint: f.s3_endpoint.clone(),
                        s3_region: f.s3_region.clone(),
                        s3_path_style: f.s3_path_style,
                    }
                }),
                auth: cfg.auth.as_ref().map(|a| egret_orchestrator::Auth {
                    password_users: a
                        .password_users
                        .iter()
                        .map(|p| egret_orchestrator::PasswordEntry {
                            username: p.username.clone(),
                            bcrypt_hash: p.bcrypt_hash.clone(),
                        })
                        .collect(),
                    jwt: a.jwt.as_ref().map(|j| egret_orchestrator::Jwt {
                        jwks_uri: j.jwks_uri.clone(),
                        required_audience: j.required_audience.clone(),
                    }),
                }),
                ingress: cfg.ingress.as_ref().map(|i| egret_orchestrator::Ingress {
                    enabled: i.enabled,
                    class_name: i.class_name.clone(),
                    host: i.host.clone(),
                    tls_secret: i.tls_secret.clone(),
                    annotations: i.annotations.clone(),
                }),
                image: cfg.image.clone(),
                image_pull_secrets: cfg.image_pull_secrets.clone(),
                image_pull_policy: cfg.image_pull_policy.clone(),
                service_account: cfg.service_account.clone(),
                pod_security_context: cfg
                    .pod_security_context
                    .as_ref()
                    .map(Self::security_context_from),
                container_security_context: cfg
                    .container_security_context
                    .as_ref()
                    .map(Self::security_context_from),
                init_containers: cfg
                    .init_containers
                    .iter()
                    .map(Self::mini_container_from)
                    .collect(),
                sidecar_containers: cfg
                    .sidecar_containers
                    .iter()
                    .map(Self::mini_container_from)
                    .collect(),
                env_from: cfg
                    .env_from
                    .iter()
                    .map(Self::env_from_source_from)
                    .collect(),
                config_mounts: cfg
                    .config_mounts
                    .iter()
                    .map(Self::object_mount_from)
                    .collect(),
                secret_mounts: cfg
                    .secret_mounts
                    .iter()
                    .map(Self::object_mount_from)
                    .collect(),
                deployment_strategy: cfg
                    .deployment_strategy
                    .as_ref()
                    .map(Self::deployment_strategy_from),
                deployment_annotations: cfg.deployment_annotations.clone(),
                revision_history_limit: cfg.revision_history_limit,
                progress_deadline_seconds: cfg.progress_deadline_seconds,
                share_process_namespace: cfg.share_process_namespace,
                autoscaling: cfg.autoscaling.as_ref().map(Self::autoscaling_from),
            },
            catalogs: req
                .catalogs
                .iter()
                .map(|c| egret_orchestrator::CatalogSpec {
                    name: c.name.clone(),
                    properties: c.properties.clone(),
                    env: c.env.clone(),
                })
                .collect(),
        }
    }
}

#[async_trait::async_trait]
impl egret_controlplane::EngineProvisioner for EgretEngineProvisioner {
    async fn provision(&self, req: &egret_controlplane::EngineProvisionReq) -> Result<(), String> {
        egret_orchestrator::EngineProvisioner::provision(self.inner.as_ref(), &Self::spec_from(req))
            .await
            .map_err(|e| e.to_string())
    }

    async fn status(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<egret_controlplane::EngineLiveStatus, String> {
        let st =
            egret_orchestrator::EngineProvisioner::status(self.inner.as_ref(), namespace, name)
                .await
                .map_err(|e| e.to_string())?;
        Ok(egret_controlplane::EngineLiveStatus {
            phase: st.phase,
            message: st.message,
            pods: st
                .pods
                .into_iter()
                .map(|p| egret_controlplane::EnginePod {
                    name: p.name,
                    phase: p.phase,
                    ready: p.ready,
                })
                .collect(),
        })
    }

    async fn update(&self, req: &egret_controlplane::EngineProvisionReq) -> Result<(), String> {
        egret_orchestrator::EngineProvisioner::update(self.inner.as_ref(), &Self::spec_from(req))
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
        // config — same seam as coordinator_request/pod_logs below.
        self.kube
            .restart_deployment(namespace, &format!("{name}-coordinator"))
            .await
            .map_err(|e| e.to_string())?;
        self.kube
            .restart_deployment(namespace, &format!("{name}-worker"))
            .await
            .map_err(|e| e.to_string())
    }

    async fn coordinator_request(
        &self,
        namespace: &str,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        self.kube
            .coordinator_request(namespace, service, method, path, headers, body)
            .await
    }

    async fn pod_logs(
        &self,
        namespace: &str,
        pod: &str,
        container: Option<&str>,
        tail_lines: i64,
        previous: bool,
    ) -> anyhow::Result<String> {
        self.kube
            .pod_logs(namespace, pod, container, tail_lines, previous)
            .await
    }

    async fn pod_logs_stream(
        &self,
        namespace: &str,
        pod: &str,
        tail_lines: i64,
    ) -> anyhow::Result<
        std::pin::Pin<Box<dyn futures_util::Stream<Item = anyhow::Result<bytes::Bytes>> + Send>>,
    > {
        self.kube.pod_logs_stream(namespace, pod, tail_lines).await
    }

    async fn exec(
        &self,
        namespace: &str,
        pod: &str,
        command: &[String],
    ) -> anyhow::Result<kube::api::AttachedProcess> {
        self.kube.exec(namespace, pod, command).await
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

    // Warn (do not fail) if any stored role permission is unknown to the catalog —
    // catches drift between the DB and egret_core::rbac.
    if let Ok(rows) =
        sqlx::query_scalar::<_, String>("SELECT DISTINCT permission FROM role_permissions")
            .fetch_all(&db.pool)
            .await
    {
        for perm in rows.iter().filter(|p| !egret_core::rbac::is_known(p)) {
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
    // engine routes then report "kubernetes not configured".
    let engine_provisioner: Option<std::sync::Arc<dyn egret_controlplane::EngineProvisioner>> = {
        let kube = match egret_orchestrator::LiveKube::in_cluster().await {
            Ok(k) => Some(k),
            Err(_) => egret_orchestrator::LiveKube::from_kubeconfig().await.ok(),
        };
        match kube {
            Some(k) => {
                let kube: std::sync::Arc<dyn egret_orchestrator::KubeClient> =
                    std::sync::Arc::new(k);
                let trino = egret_orchestrator::TrinoProvisioner::new(kube.clone());
                Some(std::sync::Arc::new(EgretEngineProvisioner::new(
                    std::sync::Arc::new(trino),
                    kube,
                )))
            }
            None => {
                tracing::warn!("kubernetes unavailable; engine provisioning disabled");
                None
            }
        }
    };

    // Control plane: global platform-admin Storage + Secrets + embedded-catalog
    // management. The warehouse provisioner (above) creates lakekeeper warehouses.
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
        engine_provisioner,
        engine_egret_url: config
            .catalog
            .public_url
            .clone()
            .unwrap_or_else(|| "http://egret:8080".to_string()),
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
