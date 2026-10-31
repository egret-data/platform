//! `egret` binary entrypoint: parse the CLI, load config, dispatch the command.

mod apps_pod_k8s;
mod backfill;
mod cli;

use anyhow::Context;
use chrono::Duration;
use clap::Parser;
use cli::{Cli, Command, WorkspaceAction};
use egret_auth::groups::GroupRepo;
use egret_auth::password::hash_password;
use egret_auth::routes::AuthState;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_bin::bootstrap::{admin_report, AdminPassword};
use egret_catalog_engine::grant_match::{catalog_grant_covers_by, grants_write, CatalogNameEq};
use egret_config::Config;
use egret_core::ratelimit::RateLimiter;
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

    // The `openapi` command is pure: emit the spec and exit before touching
    // config, tracing, or the database.
    if let Command::Openapi = cli.command {
        let doc = egret_server::openapi_doc();
        println!("{}", serde_json::to_string_pretty(&doc)?);
        return Ok(());
    }

    let config = Config::load().context("loading configuration")?;
    config.validate().map_err(|e| anyhow::anyhow!(e))?;
    init_tracing(&config);
    // After tracing, so the warnings reach the log. Warnings, not errors: a
    // stale key in a running deployment must not stop it starting (KI-140).
    for unknown in Config::unknown_keys() {
        tracing::warn!("{unknown}");
    }
    // Beside tracing, and for the same reason: both are process-wide sinks that
    // have to exist before anything that writes to them runs. Every
    // `metrics::counter!` in the tree records into a no-op recorder until this
    // line, so instrumentation that fires during boot would otherwise be
    // silently dropped — and `GET /metrics` would answer 503 while the server
    // served traffic.
    //
    // Propagated rather than warned about. The two ways this can fail are an
    // invalid bucket list (a constant in `egret-server`) and a recorder already
    // installed (impossible: nothing else installs one) — so a failure here is a
    // bug in this process, not an environment egret should carry on into.
    egret_server::install_metrics_recorder()
        .map_err(|e| anyhow::anyhow!("installing the metrics recorder: {e}"))?;

    match cli.command {
        Command::Serve => serve(config).await,
        // Handled above, before config load.
        Command::Openapi => unreachable!("openapi handled before config load"),
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
/// platform_admin already exists (unless `force` is set). Prints a generated password once,
/// and never one supplied in `EGRET_ADMIN_PASSWORD`.
async fn init(config: Config, force: bool) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations")?;

    if !force {
        let existing: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM access_grants ag \
             JOIN roles r ON r.id = ag.role_id \
             WHERE r.scope='platform' AND r.name='platform_admin' AND ag.scope_type='platform'",
        )
        .fetch_one(&db.pool)
        .await?;
        if existing > 0 {
            tracing::warn!("a platform_admin already exists; use --force to add another");
            return Ok(());
        }
    }

    let email = std::env::var("EGRET_ADMIN_EMAIL").unwrap_or_else(|_| "admin@egret.local".into());
    let password = AdminPassword::from_env(
        std::env::var("EGRET_ADMIN_PASSWORD").ok(),
        egret_auth::token::generate_token,
    );
    let hash = hash_password(password.secret())?;

    let users = UserRepo::new(db.clone());
    let user_id = users.insert_local(&email, "Administrator", &hash).await?;

    // NOT EXISTS rather than ON CONFLICT: platform grants have a NULL scope_id,
    // which the UNIQUE index treats as distinct, so ON CONFLICT would not dedupe.
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, ro.id, 'platform', NULL FROM roles ro \
         WHERE ro.scope='platform' AND ro.name='platform_admin' \
           AND NOT EXISTS ( \
             SELECT 1 FROM access_grants ag \
             WHERE ag.principal_type='user' AND ag.principal_id=$1 \
               AND ag.role_id=ro.id AND ag.scope_type='platform')",
    )
    .bind(user_id.0)
    .execute(&db.pool)
    .await?;

    // Username takeover (spec 2026-10-23 §4.2): with `--force` on an install
    // that already has engines, the new admin joins every `egret-admin` set
    // now rather than at the next boot.
    if let Err(e) = egret_perm_compile::sync_admin_policies(&db.pool).await {
        tracing::warn!(error = %e, "could not add the new admin to the engines' egret-admin \
            policies; the boot pass will");
    }

    tracing::info!(%email, "bootstrap admin created");
    // KI-146: a supplied password is never echoed into scroll-back or CI logs.
    println!("{}", admin_report(&email, &password));

    // Seed a default workspace so a fresh deploy has somewhere to provision
    // warehouses (a bare `egret init` otherwise leaves 0 workspaces, and
    // warehouse-creation has nothing to attach to). Idempotent: reuse an
    // existing `default` workspace rather than erroring on a re-run
    // (`init --force`, or a partial prior run). The freshly-bootstrapped admin
    // is made the workspace owner (`ws_admin`) so it can actually use it;
    // `MemberRepo::add` replaces-then-inserts, so re-running is a no-op.
    let workspaces = WorkspaceRepo::new(db.clone());
    let default_ws = match workspaces
        .get_by_slug("default")
        .await
        .context("looking up default workspace")?
    {
        Some(ws) => ws,
        None => workspaces
            .create("default", "Default", user_id)
            .await
            .context("creating default workspace")?,
    };
    MemberRepo::new(db.clone())
        .add(default_ws.id, "user", user_id.0, "ws_admin", user_id)
        .await
        .context("adding bootstrap admin as default-workspace owner")?;

    tracing::info!(slug = "default", "default workspace ready");
    println!("Default workspace: default");
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

/// Reads the engine-provisioning seam for the Policy/Ranger admin console (E2d):
/// which provisioned engines bind to which Ranger profile, and their catalog
/// names. Backed by `EngineMgmtStore`; the composition root is the only place the
/// two seams meet (the Policies module depends on `EngineBindingReader`, not on
/// `egret-engine-mgmt`).
/// The engine seam's `ProfileCompilerPort`, implemented here in the
/// composition root (SP4a Task 9 review).
///
/// It cannot live in `egret-engine-mgmt`: the compiler is `egret-perm-compile`,
/// which depends on `egret-policy`, and keeping `egret-policy`'s internals out
/// of the engine seam's production graph is the whole reason
/// `egret-policy-api::PolicyPort` exists there. Same shape, opposite direction,
/// as `EngineBindingReaderImpl` below.
struct ProfileCompilerImpl {
    perm: egret_perm::PermStore,
    policy: egret_policy::PolicyStore,
}

/// Shared body for both ports' `recompile_catalog_assignment_in_tx`
/// (`egret_engine_mgmt::ProfileCompilerPort` and
/// `egret_catalog_registry::CatalogAssignmentCompiler` — SP4a Task 16): one
/// implementation of "recompile every profile this catalog reaches," not two
/// that can drift, since `ProfileCompilerImpl` implements both ports and
/// neither method touches `self`.
///
/// The same affected-service derivation `affected_services_for_grant` uses
/// for a grant scoped to this catalog: every profile an engine currently
/// binds that also attaches this catalog. `services_for_catalog_via` already
/// emits `SELECT DISTINCT ... ORDER BY 1`, but the sort/dedup is redone here
/// explicitly rather than trusted to the query, matching
/// `egret_perm_compile::trigger::normalize_service_order`'s own reasoning:
/// `lock_services_in_tx`'s ascending-order invariant is load-bearing (it is
/// what turns two overlapping recompiles into a queue instead of a
/// deadlock), so the guarantee lives at this call site too.
async fn recompile_catalog_assignment(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    catalog_id: uuid::Uuid,
) -> Result<(), egret_perm::CompileError> {
    let mut ids = egret_policy::services_for_catalog_via(tx, catalog_id)
        .await
        .map_err(|e| egret_perm::CompileError::Db(e.to_string()))?;
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }
    egret_perm_compile::lock_services_in_tx(tx, &ids).await?;
    egret_perm_compile::recompile_services_in_tx(tx, &ids).await?;
    Ok(())
}

/// Lock + recompile exactly ONE profile on the caller's transaction.
///
/// `lock_services_in_tx` first, for the same reason
/// [`recompile_catalog_assignment`] takes it: it is the single gate that
/// expands the set with each profile's TAG service
/// (`egret_policy::services_and_tag_services_via` — a tag service's id can be
/// LOWER than its profile's, 51 vs 52 on the live cluster) and locks the
/// expanded set ASCENDING, which is what keeps two overlapping recompiles
/// queueing instead of deadlocking. A one-element slice is trivially sorted,
/// so the `debug_assert!(service_ids.is_sorted())` inside that gate holds; the
/// expansion it performs is the part that matters here.
async fn recompile_one_profile(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    service_id: i64,
) -> Result<(), egret_perm::CompileError> {
    egret_perm_compile::lock_services_in_tx(tx, &[service_id]).await?;
    egret_perm_compile::recompile_service_in_tx(tx, service_id).await?;
    Ok(())
}

#[async_trait::async_trait]
impl egret_engine_mgmt::ProfileCompilerPort for ProfileCompilerImpl {
    async fn recompile_profile(&self, service_id: i64) -> Result<(), egret_perm::CompileError> {
        egret_perm_compile::recompile_service(&self.perm, &self.policy, service_id)
            .await
            .map(|_| ())
    }

    async fn recompile_catalog_assignment_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        catalog_id: uuid::Uuid,
    ) -> Result<(), egret_perm::CompileError> {
        recompile_catalog_assignment(tx, catalog_id).await
    }

    async fn recompile_profile_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_id: i64,
    ) -> Result<(), egret_perm::CompileError> {
        recompile_one_profile(tx, service_id).await
    }

    async fn overlap_snapshot_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_ids: &[i64],
    ) -> Result<egret_engine_mgmt::OverlapSnapshot, egret_perm::CompileError> {
        Ok(egret_engine_mgmt::OverlapSnapshot::new(
            egret_perm_compile::overlap::unit_snapshot_via(tx, service_ids).await?,
        ))
    }

    async fn refuse_created_overlaps_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        snapshot: &egret_engine_mgmt::OverlapSnapshot,
    ) -> Result<(), egret_perm::CompileError> {
        let snapshot = snapshot
            .get::<egret_perm_compile::overlap::UnitSnapshot>()
            .ok_or_else(|| {
                egret_perm::CompileError::Db(
                    "an overlap snapshot of a type this compiler did not produce".into(),
                )
            })?;
        egret_perm_compile::overlap::refuse_created_overlaps_via(tx, snapshot).await
    }
}

/// `egret-catalog-registry`'s narrower port (SP4a Task 16, D3 defects 2 and
/// 3): that crate deliberately carries no `egret-perm-compile`/`egret-policy`
/// dependency (see `egret_catalog_registry::compiler_port`'s module doc), so
/// this is a SEPARATE `impl` block on the same concrete struct rather than a
/// supertrait — `ProfileCompilerImpl` satisfies both ports, and each caller
/// (`egret-engine-mgmt`, `egret-catalog-registry`/`egret-catalog-engine`)
/// only ever sees the one its own crate is allowed to depend on.
#[async_trait::async_trait]
impl egret_catalog_registry::CatalogAssignmentCompiler for ProfileCompilerImpl {
    async fn recompile_catalog_assignment_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        catalog_id: uuid::Uuid,
    ) -> Result<(), egret_perm::CompileError> {
        recompile_catalog_assignment(tx, catalog_id).await
    }

    async fn recompile_services_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_ids: &[i64],
    ) -> Result<(), egret_perm::CompileError> {
        if service_ids.is_empty() {
            return Ok(());
        }
        egret_perm_compile::recompile_services_in_tx(tx, service_ids).await?;
        Ok(())
    }
}

struct EngineBindingReaderImpl {
    store: egret_engine_mgmt::EngineMgmtStore,
    /// Resolves a warehouse's lakekeeper id + browses its catalog metadata
    /// (schemas/tables/columns) for resource autocomplete. Both `None` when the
    /// embedded catalog is disabled — the metadata methods then return empty.
    catalog: Option<egret_catalog_engine::CatalogStore>,
    browser: Option<std::sync::Arc<dyn egret_catalog_engine::WarehouseBrowser>>,
}

impl EngineBindingReaderImpl {
    /// egret warehouse id → its lakekeeper warehouse id (the browse key), or
    /// `None` if the catalog is disabled / the warehouse isn't catalog-backed.
    async fn resolve_lk(
        &self,
        warehouse_id: uuid::Uuid,
    ) -> Result<Option<String>, egret_policy::EngineReadError> {
        let Some(cat) = &self.catalog else {
            return Ok(None);
        };
        Ok(cat
            .get_warehouse(warehouse_id)
            .await
            .map_err(|e| egret_policy::EngineReadError(e.to_string()))?
            .and_then(|w| w.lakekeeper_warehouse_id))
    }
}

#[async_trait::async_trait]
impl egret_policy::EngineBindingReader for EngineBindingReaderImpl {
    async fn list_engine_bindings(
        &self,
    ) -> Result<Vec<egret_policy::EngineBinding>, egret_policy::EngineReadError> {
        let engines = self
            .store
            .list_engines()
            .await
            .map_err(|e| egret_policy::EngineReadError(e.to_string()))?;
        // Warehouses come from the ATTACHMENTS, not from `engines.warehouse_id`
        // (SP3c-2 Task 5 left that column unread; Task 6 drops it). One query
        // for all engines, so an autocomplete keystroke is not N+1.
        let ids: Vec<uuid::Uuid> = engines.iter().map(|e| e.id).collect();
        let mut warehouses = self
            .store
            .registry()
            .warehouse_ids_by_engine(&ids)
            .await
            .map_err(|e| egret_policy::EngineReadError(e.to_string()))?;
        Ok(engines
            .into_iter()
            .map(|e| egret_policy::EngineBinding {
                warehouse_ids: warehouses.remove(&e.id).unwrap_or_default(),
                id: e.id,
                name: e.name,
                status: e.status,
                origin: e.origin,
                r#type: e.r#type,
                authz_service_id: e.authz_service_id,
            })
            .collect())
    }

    async fn engine_catalog_names(
        &self,
        engine_id: uuid::Uuid,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        self.store
            .registry()
            .attached_catalog_names(engine_id)
            .await
            .map_err(|e| egret_policy::EngineReadError(e.to_string()))
    }

    async fn warehouse_schemas(
        &self,
        warehouse_id: uuid::Uuid,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        let (Some(lk), Some(browser)) = (self.resolve_lk(warehouse_id).await?, &self.browser)
        else {
            return Ok(Vec::new());
        };
        let ns = browser
            .namespaces(&lk)
            .await
            .map_err(|e| egret_policy::EngineReadError(e.to_string()))?;
        Ok(ns.into_iter().map(|n| n.parts.join(".")).collect())
    }

    async fn warehouse_tables(
        &self,
        warehouse_id: uuid::Uuid,
        schema: &str,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        let (Some(lk), Some(browser)) = (self.resolve_lk(warehouse_id).await?, &self.browser)
        else {
            return Ok(Vec::new());
        };
        let ns: Vec<String> = schema.split('.').map(str::to_string).collect();
        let tabs = browser
            .tabulars(&lk, &ns)
            .await
            .map_err(|e| egret_policy::EngineReadError(e.to_string()))?;
        Ok(tabs.into_iter().map(|t| t.name).collect())
    }

    async fn warehouse_columns(
        &self,
        warehouse_id: uuid::Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        let (Some(lk), Some(browser)) = (self.resolve_lk(warehouse_id).await?, &self.browser)
        else {
            return Ok(Vec::new());
        };
        let ns: Vec<String> = schema.split('.').map(str::to_string).collect();
        let detail = browser
            .schema(&lk, &ns, table)
            .await
            .map_err(|e| egret_policy::EngineReadError(e.to_string()))?;
        Ok(detail.columns.into_iter().map(|c| c.name).collect())
    }
}

/// The only `aud` this guard accepts. The machine-IdP signs several audiences
/// with one key (`CatalogStore` mints `catalog`, `ClientStore` mints
/// `data-api`), so the audience — not the signature — is what says a token was
/// meant for the catalog.
const CATALOG_AUDIENCE: &str = "catalog";

/// egret's authN + authZ guard for the embedded catalog (EB-3). Validates the
/// egret-issued JWT (authN); the JWT's `warehouse`+`roles` claims + the RBAC
/// store drive authZ. lakekeeper itself runs on `AllowAllAuthorizer` — egret owns
/// the decision here.
struct EgretCatalogGuard {
    keys: egret_machine_idp::PgSigningKeyStore,
    catalog: egret_catalog_engine::CatalogStore,
}

impl EgretCatalogGuard {
    /// authN: a valid, unexpired egret JWT minted for the catalog audience.
    /// Returns its claims.
    async fn verify(
        &self,
        bearer: Option<&str>,
    ) -> Result<serde_json::Value, (axum::http::StatusCode, String)> {
        use axum::http::StatusCode;
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
        // The machine-IdP signs SEVERAL audiences with this one key, and
        // `verify_with_jwk` sets `validate_aud = false` — so a signature check
        // alone proves the token came from egret, not that it was meant for the
        // catalog. Since Data API P2 a `ws_admin` can self-issue a `data-api`
        // token from the same key; without this the data-plane arms below would
        // still fail closed (an empty `warehouse` claim matches no warehouse),
        // but `GET /config` with no `warehouse=` returns `Ok(())` and reaches
        // lakekeeper. Assert the audience here rather than relying on every
        // downstream arm to fail closed by accident.
        if claims.get("aud").and_then(|v| v.as_str()) != Some(CATALOG_AUDIENCE) {
            return Err((StatusCode::UNAUTHORIZED, "wrong audience".to_string()));
        }
        Ok(claims)
    }
}

#[async_trait::async_trait]
impl egret_catalog_engine::CatalogGuard for EgretCatalogGuard {
    async fn authenticate(
        &self,
        bearer: Option<&str>,
    ) -> Result<(), (axum::http::StatusCode, String)> {
        self.verify(bearer).await.map(|_| ())
    }

    async fn check(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        bearer: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<egret_catalog_engine::CatalogCaller, (axum::http::StatusCode, String)> {
        use axum::http::StatusCode;
        let forbid = |m: String| Err((StatusCode::FORBIDDEN, m));
        let claims = self.verify(bearer).await?;
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
        // Who sent it (rename spec R7): a layer behind the guard names the
        // caller from this, not from a claim it would have to verify again,
        // and lakekeeper's authorizer scopes vended credentials to its role's
        // grants (vended-credentials spec §4.4).
        let caller = egret_catalog_engine::CatalogCaller {
            subject: claims
                .get("sub")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            audience: CATALOG_AUDIENCE.to_string(),
            warehouse: token_warehouse.to_string(),
            role: token_role.to_string(),
        };

        // Remote S3 request signing (`/aws/s3/sign`, `/{prefix}/v1/aws/s3/sign`,
        // `/signer/{prefix}/tabular-id/{id}/v1/aws/s3/sign`): egret's warehouses
        // have it off and no egret client uses it, and lakekeeper's own table
        // check behind it allows everyone, so refuse every signer route here,
        // whatever the grants (rename-authz spec §10.6).
        if is_signer_route(path) {
            return forbid(
                "remote signing is not used by egret: S3 access goes through vended credentials"
                    .to_string(),
            );
        }

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
            return Ok(caller);
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
            let malformed =
                |what: &str, m: String| (StatusCode::BAD_REQUEST, format!("malformed {what}: {m}"));

            // What this request must be granted, each with its refusal. A rename
            // (`/tables/rename`, `/views/rename`) names what it touches only in
            // its body: it needs what a drop of the source needs and what a
            // create in the destination's namespace needs (rename-authz spec
            // §3.2). A multi-table commit (`/transactions/commit`) needs, for
            // every table it changes, what an update of that table alone needs
            // (§8.2). A single-table commit needs write on the table lakekeeper
            // commits to, which its body may name (§10.3). Any other request is
            // decided on its path.
            let mut needs: Vec<Need> = Vec::new();
            match egret_catalog_engine::GuardedBody::of(method, path) {
                Some(egret_catalog_engine::GuardedBody::Rename) => {
                    let (src, dst) =
                        parse_rename_body(body).map_err(|m| malformed("rename request", m))?;
                    needs.push(Need {
                        refusal: format!(
                            "role '{token_role}' may not rename {}.{}: renaming needs write on the \
                             source, as dropping it would",
                            src.0.join("."),
                            src.1
                        ),
                        ns: src.0,
                        table: Some(src.1),
                        write: true,
                    });
                    let ns = dst.0.join(".");
                    needs.push(Need {
                        refusal: format!(
                            "role '{token_role}' may not rename into {ns}.{}: renaming needs write on \
                             namespace {ns}, as creating a table there would",
                            dst.1
                        ),
                        ns: dst.0,
                        table: None,
                        write: true,
                    });
                }
                Some(egret_catalog_engine::GuardedBody::MultiTableCommit) => {
                    let tables =
                        parse_commit_body(body).map_err(|m| malformed("multi-table commit", m))?;
                    for (ns, name) in tables {
                        needs.push(Need {
                            refusal: format!(
                                "role '{token_role}' may not commit to {}.{name}: a multi-table \
                                 commit needs write on every table it changes, as updating it \
                                 alone would; nothing was committed",
                                ns.join(".")
                            ),
                            ns,
                            table: Some(name),
                            write: true,
                        });
                    }
                }
                Some(egret_catalog_engine::GuardedBody::TableCommit) => {
                    // lakekeeper commits to the body's `identifier` whenever
                    // it accepts one (its "Spark branch" fallback included),
                    // and to the path's table otherwise: authorize that table.
                    let named =
                        parse_table_commit_body(body).map_err(|m| malformed("table commit", m))?;
                    let (ns, name) = match named {
                        Some(ident) => ident,
                        None => (
                            path_namespace(segs[2])
                                .ok_or_else(|| malformed("path", "bad percent-encoding".into()))?,
                            path_tabular(segs[4])
                                .ok_or_else(|| malformed("path", "bad percent-encoding".into()))?,
                        ),
                    };
                    needs.push(Need {
                        refusal: format!(
                            "role '{token_role}' may not commit to {}.{name}: it is not granted \
                             write on that table",
                            ns.join(".")
                        ),
                        ns,
                        table: Some(name),
                        write: true,
                    });
                }
                None => {
                    // Resolve the request target: /namespaces/{ns}[/tables|views/{table}],
                    // decoded as lakekeeper decodes its path parameters.
                    let refusal = format!(
                        "role '{token_role}' is not granted {} on the requested resource",
                        if is_write { "write" } else { "read" }
                    );
                    let target_ns = match (segs.get(1), segs.get(2)) {
                        (Some(&"namespaces"), Some(seg)) => Some(
                            path_namespace(seg)
                                .ok_or_else(|| malformed("path", "bad percent-encoding".into()))?,
                        ),
                        _ => None,
                    };
                    let target_table = match (segs.get(3), segs.get(4)) {
                        (Some(&"tables") | Some(&"views"), Some(seg)) => Some(
                            path_tabular(seg)
                                .ok_or_else(|| malformed("path", "bad percent-encoding".into()))?,
                        ),
                        _ => None,
                    };
                    // A recursive namespace drop deletes every child namespace
                    // and every table in them (§10.5): only a catalog-level
                    // write/manage grant covers all of that.
                    if method == "DELETE"
                        && target_ns.is_some()
                        && target_table.is_none()
                        && segs.len() == 3
                        && is_recursive(query)
                        && !grants
                            .iter()
                            .any(|g| g.securable_type == "catalog" && grants_write(g))
                    {
                        return forbid(format!(
                            "role '{token_role}' may not drop namespace {} recursively: a \
                             recursive drop deletes every child namespace and table, so it needs a \
                             catalog-level write or manage grant",
                            target_ns.as_deref().unwrap_or_default().join(".")
                        ));
                    }
                    match target_ns {
                        // catalog-root op (list namespaces, …): any grant for
                        // reads, a write/manage grant for writes.
                        None => {
                            if !grants.iter().any(|g| !is_write || grants_write(g)) {
                                return forbid(refusal);
                            }
                        }
                        Some(ns) => needs.push(Need {
                            ns,
                            table: target_table,
                            write: is_write,
                            refusal,
                        }),
                    }
                }
            }

            // Names compare as lakekeeper compares them (§10.4).
            let requested: Vec<&str> = needs
                .iter()
                .flat_map(|n| n.ns.iter().map(String::as_str).chain(n.table.as_deref()))
                .collect();
            let eq = CatalogNameEq::load(&self.catalog, &grants, &requested)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("names: {e}")))?;
            for need in needs {
                if !grants.iter().any(|g| {
                    catalog_grant_covers_by(
                        g,
                        Some(&need.ns),
                        need.table.as_deref(),
                        need.write,
                        &|a, b| eq.eq(a, b),
                    )
                }) {
                    return forbid(need.refusal);
                }
            }
        }
        Ok(caller)
    }
}

/// A table or view named in a request body: the namespace's levels and the
/// name.
type TableIdent = (Vec<String>, String);

/// The `{source, destination}` of a rename body, each `{namespace: [..],
/// name}`. `Err` says what is wrong, for the 400.
fn parse_rename_body(body: Option<&[u8]>) -> Result<(TableIdent, TableIdent), String> {
    let body = body.ok_or("no body")?;
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("the body is not JSON: {e}"))?;
    let side = |k: &str| -> Result<TableIdent, String> {
        identifier(json.get(k).ok_or(format!("no `{k}`"))?, k)
    };
    Ok((side("source")?, side("destination")?))
}

/// The tables a multi-table commit changes: each `table-changes[].identifier`
/// (`{namespace: [..], name}`). `Err` says what is wrong, for the 400.
fn parse_commit_body(body: Option<&[u8]>) -> Result<Vec<TableIdent>, String> {
    let body = body.ok_or("no body")?;
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("the body is not JSON: {e}"))?;
    let changes = json
        .get("table-changes")
        .and_then(|c| c.as_array())
        .ok_or("`table-changes` is not a list")?;
    if changes.is_empty() {
        return Err("`table-changes` is empty".into());
    }
    changes
        .iter()
        .enumerate()
        .map(|(i, c)| {
            c.get("identifier")
                .ok_or(format!("`table-changes[{i}]` has no `identifier`"))
                .and_then(|id| identifier(id, &format!("table-changes[{i}].identifier")))
        })
        .collect()
}

/// The table a single-table commit's body names in `identifier`, if any.
/// `Err` says what is wrong, for the 400.
fn parse_table_commit_body(body: Option<&[u8]>) -> Result<Option<TableIdent>, String> {
    let body = body.ok_or("no body")?;
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("the body is not JSON: {e}"))?;
    match json.get("identifier") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(id) => identifier(id, "identifier").map(Some),
    }
}

/// `{namespace: [..], name}`: the namespace a non-empty list of strings, the
/// name a non-empty string. `at` names it in the error.
fn identifier(v: &serde_json::Value, at: &str) -> Result<TableIdent, String> {
    let ns: Vec<String> = v
        .get("namespace")
        .and_then(|n| n.as_array())
        .and_then(|a| {
            a.iter()
                .map(|l| l.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|ns| !ns.is_empty())
        .ok_or(format!(
            "`{at}.namespace` is not a non-empty list of strings"
        ))?;
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())
        .ok_or(format!("`{at}.name` is not a non-empty string"))?;
    Ok((ns, name.to_string()))
}

/// One thing a request must be granted, and what the 403 says if not.
struct Need {
    ns: Vec<String>,
    table: Option<String>,
    write: bool,
    refusal: String,
}

/// Percent-decode one path segment, as axum's `Path` does for lakekeeper.
/// `None` for bad percent-encoding or bytes that are not UTF-8.
fn percent_decode(seg: &str) -> Option<String> {
    let bytes = seg.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A namespace path segment, as lakekeeper reads it (`NamespaceIdentUrl`):
/// percent-decoded, split on the unit separator (`\x1f`, `%1F` in the URL),
/// each level with `+` read as a space.
fn path_namespace(seg: &str) -> Option<Vec<String>> {
    Some(
        percent_decode(seg)?
            .split('\u{1f}')
            .map(|s| s.replace('+', " "))
            .collect(),
    )
}

/// A table or view path segment, as lakekeeper reads it
/// (`normalize_tabular_name`): percent-decoded, `+` read as a space.
fn path_tabular(seg: &str) -> Option<String> {
    Some(percent_decode(seg)?.replace('+', " "))
}

/// Does the query ask for a recursive drop? Keys and values are
/// form-decoded as lakekeeper's query parser decodes them; any `recursive`
/// whose value is not `false` counts (lakekeeper refuses values other than
/// `true`/`false`, so this only ever refuses more).
fn is_recursive(query: Option<&str>) -> bool {
    query.is_some_and(|q| {
        url::form_urlencoded::parse(q.as_bytes()).any(|(k, v)| k == "recursive" && v != "false")
    })
}

/// Is this one of lakekeeper's S3 signer routes? Any path ending in
/// `aws/s3/sign`, or under `/signer/`.
fn is_signer_route(path: &str) -> bool {
    let segs: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    segs.first() == Some(&"signer") || segs.ends_with(&["aws", "s3", "sign"])
}

/// [`catalog_grant_covers_by`] with names compared byte for byte (the
/// structural unit tests).
#[cfg(test)]
fn catalog_grant_covers(
    g: &egret_catalog_engine::CatalogGrant,
    target_ns: Option<&[String]>,
    target_table: Option<&str>,
    is_write: bool,
) -> bool {
    catalog_grant_covers_by(g, target_ns, target_table, is_write, &|a, b| a == b)
}

/// Provisions the embedded lakekeeper warehouse for an egret `warehouses` row.
/// Holds the lakekeeper `ApiContext` (built in `serve`).
struct LakekeeperProvisioner {
    engine_state: egret_catalog_engine::ApiContext<egret_catalog_engine::EngineState>,
    system_user: String,
}

#[async_trait::async_trait]
impl egret_catalog_engine::WarehouseProvisioner for LakekeeperProvisioner {
    async fn provision(
        &self,
        name: &str,
        s3: &egret_catalog_engine::ProvisionS3,
    ) -> Result<String, egret_catalog_engine::CatalogEngineError> {
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
    }
}

/// Which engine provisioner `[engines]` selects.
#[derive(Debug, PartialEq, Eq)]
enum Selection {
    Kubernetes,
    Docker,
    Disabled,
}

/// Pure so the rules are testable: `auto` is Kubernetes when a client could be
/// built and disabled otherwise (today's behaviour); `docker` is only ever
/// chosen explicitly, whatever the cluster; an explicit `kubernetes` without a
/// client is a startup error rather than a silent downgrade.
fn select_provisioner(
    cfg: &egret_config::EnginesConfig,
    kube_available: bool,
) -> Result<Selection, String> {
    use egret_config::Provisioner;
    match (cfg.provisioner, kube_available) {
        (Provisioner::Auto, true) => Ok(Selection::Kubernetes),
        (Provisioner::Auto, false) => Ok(Selection::Disabled),
        (Provisioner::Kubernetes, true) => Ok(Selection::Kubernetes),
        (Provisioner::Kubernetes, false) => Err(
            "engines.provisioner = \"kubernetes\" but no kubernetes client could be built (neither in-cluster nor kubeconfig)"
                .into(),
        ),
        (Provisioner::Docker, _) => Ok(Selection::Docker),
        (Provisioner::None, _) => Ok(Selection::Disabled),
    }
}

/// Provisions query engines (Phase 1: Trino, Phase 2: StarRocks) onto
/// Kubernetes for the engine-mgmt seam. Adapts the kube-free
/// `egret_engine_mgmt_api::EngineProvisioningPort` trait onto the
/// `egret-provisioner` provisioners, dispatching by engine kind. Since E1a
/// both traits' spec/status types are the SAME `egret-engine-mgmt-api` DTOs
/// (aliased under different names on each side), so this is a pure
/// pass-through — no translation between the two. The 4 kube-shaped ops
/// (exec/pod-logs/coordinator-request) are NOT on the Port — the seam calls
/// `egret_provisioner::KubeClient` directly via `EngineMgmtState.kube`.
struct EgretEngineProvisioningPort {
    trino: std::sync::Arc<egret_provisioner::TrinoProvisioner>,
    starrocks: std::sync::Arc<egret_provisioner::StarRocksProvisioner>,
    /// The same KubeClient both provisioners hold, kept here so Trino's
    /// `restart` (which bypasses TrinoProvisioner's render/apply path) can
    /// reach it directly.
    kube: std::sync::Arc<dyn egret_provisioner::KubeClient>,
}

impl EgretEngineProvisioningPort {
    fn new(
        trino: std::sync::Arc<egret_provisioner::TrinoProvisioner>,
        starrocks: std::sync::Arc<egret_provisioner::StarRocksProvisioner>,
        kube: std::sync::Arc<dyn egret_provisioner::KubeClient>,
    ) -> Self {
        Self {
            trino,
            starrocks,
            kube,
        }
    }

    /// Picks the provisioner for a given `EngineKind::as_str()` value.
    fn for_kind(&self, kind: &str) -> &dyn egret_provisioner::EngineProvisioner {
        match kind {
            "starrocks" => &*self.starrocks,
            _ => &*self.trino,
        }
    }
}

#[async_trait::async_trait]
impl egret_engine_mgmt_api::EngineProvisioningPort for EgretEngineProvisioningPort {
    async fn provision(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        self.for_kind(req.kind.as_str())
            .provision(req)
            .await
            .map_err(|e| e.to_string())
    }

    async fn status(
        &self,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> Result<egret_engine_mgmt_api::EngineStatus, String> {
        // `egret_engine_mgmt_api::EngineStatus` and `egret_provisioner::EngineStatus`
        // are now the SAME type (both alias `egret_engine_mgmt_api::EngineStatus`), so
        // the provisioner's status passes straight through — no reverse mapping.
        self.for_kind(kind)
            .status(namespace, name)
            .await
            .map_err(|e| e.to_string())
    }

    async fn update(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        self.for_kind(req.kind.as_str())
            .update(req)
            .await
            .map_err(|e| e.to_string())
    }

    async fn delete(&self, kind: &str, namespace: &str, name: &str) -> Result<(), String> {
        self.for_kind(kind)
            .delete(namespace, name)
            .await
            .map_err(|e| e.to_string())
    }

    async fn ranger_rollout(
        &self,
        kind: &str,
        namespace: &str,
        name: &str,
        credential_id: &str,
    ) -> Result<egret_engine_mgmt_api::RangerRollout, String> {
        self.for_kind(kind)
            .ranger_rollout(namespace, name, credential_id)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    async fn delete_ranger_secret(
        &self,
        kind: &str,
        namespace: &str,
        name: &str,
        credential_id: &str,
    ) -> Result<(), String> {
        self.for_kind(kind)
            .delete_ranger_secret(namespace, name, credential_id)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    async fn restart(&self, kind: &str, namespace: &str, name: &str) -> Result<(), String> {
        if kind == "starrocks" {
            // StarRocks pods are owned by the kube-starrocks operator (not plain
            // Deployments), so we roll them THROUGH the CR: merge-patch a fresh
            // restart-stamp annotation onto the FE + CN specs and let the operator
            // propagate it to the pod templates. The stamp is minted here because
            // egret-bin has `std::time` (the provisioner stays deterministic).
            // Use nanos (not millis) so two back-to-back restarts within the same
            // millisecond still produce a distinct value — an identical stamp would
            // leave the annotation unchanged and the operator wouldn't roll.
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos().to_string())
                .unwrap_or_else(|_| "0".to_string());
            return self
                .starrocks
                .restart(namespace, name, &stamp)
                .await
                .map_err(|e| e.to_string());
        }
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
    /// An undrop reaches lakekeeper in process, past the Iceberg mount; it goes
    /// through the table-commit hook so a classified table gets its masks back.
    table_hook: std::sync::Arc<egret_catalog_engine::TableHook>,
}

#[async_trait::async_trait]
impl egret_catalog_engine::WarehouseBrowser for LakekeeperBrowser {
    async fn namespaces(
        &self,
        wh: &str,
    ) -> Result<Vec<egret_catalog_engine::BrowseNamespace>, egret_catalog_engine::CatalogEngineError>
    {
        egret_catalog_engine::list_namespaces(self.engine_state.clone(), &self.system_user, wh)
            .await
            .map(|v| {
                v.into_iter()
                    .map(|n| egret_catalog_engine::BrowseNamespace {
                        parts: n.parts,
                        id: n.id,
                        protected: n.protected,
                    })
                    .collect()
            })
    }

    async fn tabulars(
        &self,
        wh: &str,
        ns: &[String],
    ) -> Result<Vec<egret_catalog_engine::BrowseTabular>, egret_catalog_engine::CatalogEngineError>
    {
        egret_catalog_engine::list_tabulars(self.engine_state.clone(), &self.system_user, wh, ns)
            .await
            .map(|v| {
                v.into_iter()
                    .map(|t| egret_catalog_engine::BrowseTabular {
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
    }

    async fn schema(
        &self,
        wh: &str,
        ns: &[String],
        table: &str,
    ) -> Result<egret_catalog_engine::BrowseTableDetail, egret_catalog_engine::CatalogEngineError>
    {
        egret_catalog_engine::load_table_detail(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            ns,
            table,
        )
        .await
        .map(|d| egret_catalog_engine::BrowseTableDetail {
            location: d.location,
            format_version: d.format_version,
            last_updated_ms: d.last_updated_ms,
            current_snapshot_id: d.current_snapshot_id,
            snapshot_count: d.snapshot_count,
            columns: d
                .columns
                .into_iter()
                .map(|c| egret_catalog_engine::BrowseColumn {
                    name: c.name,
                    type_: c.r#type,
                    required: c.required,
                })
                .collect(),
            partition_fields: d
                .partition_fields
                .into_iter()
                .map(|p| egret_catalog_engine::BrowsePartition {
                    name: p.name,
                    source_column: p.source_column,
                    transform: p.transform,
                })
                .collect(),
            sort_fields: d
                .sort_fields
                .into_iter()
                .map(|s| egret_catalog_engine::BrowseSort {
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
                .map(|s| egret_catalog_engine::BrowseSnapshot {
                    snapshot_id: s.snapshot_id,
                    parent_snapshot_id: s.parent_snapshot_id,
                    timestamp_ms: s.timestamp_ms,
                    operation: s.operation,
                    summary: s.summary,
                })
                .collect(),
        })
    }

    async fn view_columns(
        &self,
        wh: &str,
        ns: &[String],
        view: &str,
    ) -> Result<Vec<egret_catalog_engine::BrowseColumn>, egret_catalog_engine::CatalogEngineError>
    {
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
                .map(|c| egret_catalog_engine::BrowseColumn {
                    name: c.name,
                    type_: c.r#type,
                    required: c.required,
                })
                .collect()
        })
    }

    async fn deleted_tabulars(
        &self,
        wh: &str,
    ) -> Result<
        Vec<egret_catalog_engine::BrowseDeletedTabular>,
        egret_catalog_engine::CatalogEngineError,
    > {
        egret_catalog_engine::list_deleted_tabulars(
            self.engine_state.clone(),
            &self.system_user,
            wh,
        )
        .await
        .map(|v| {
            v.into_iter()
                .map(|d| egret_catalog_engine::BrowseDeletedTabular {
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
    }

    async fn undrop(
        &self,
        wh: &str,
        kind: &str,
        id: uuid::Uuid,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        let k = match kind {
            "view" => egret_catalog_engine::TabularKindName::View,
            _ => egret_catalog_engine::TabularKindName::Table,
        };
        self.table_hook
            .undrop(self.engine_state.clone(), &self.system_user, wh, k, id)
            .await
    }

    async fn set_protection(
        &self,
        wh: &str,
        target: &str,
        id: uuid::Uuid,
        protected: bool,
    ) -> Result<bool, egret_catalog_engine::CatalogEngineError> {
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
    }

    async fn config(
        &self,
        wh: &str,
    ) -> Result<egret_catalog_engine::BrowseWarehouseConfig, egret_catalog_engine::CatalogEngineError>
    {
        egret_catalog_engine::get_warehouse_config(self.engine_state.clone(), &self.system_user, wh)
            .await
            .map(|c| egret_catalog_engine::BrowseWarehouseConfig {
                active: c.active,
                delete_profile_seconds: c.delete_profile_seconds,
                num_tables: c.num_tables,
                num_views: c.num_views,
                stats_updated_ms: c.stats_updated_ms,
            })
    }

    async fn set_active(
        &self,
        wh: &str,
        active: bool,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        egret_catalog_engine::set_warehouse_active(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            active,
        )
        .await
    }

    async fn set_delete_profile(
        &self,
        wh: &str,
        seconds: Option<i64>,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        egret_catalog_engine::set_delete_profile(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            seconds,
        )
        .await
    }

    async fn set_format_versions(
        &self,
        wh: &str,
        versions: Vec<u8>,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        egret_catalog_engine::set_format_versions(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            &versions,
        )
        .await
    }

    async fn rotate_credential(
        &self,
        wh: &str,
        access_key: &str,
        secret_key: &str,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        egret_catalog_engine::rotate_storage_credential(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            access_key,
            secret_key,
        )
        .await
    }

    async fn search(
        &self,
        wh: &str,
        query: &str,
    ) -> Result<
        Vec<egret_catalog_engine::BrowseSearchResult>,
        egret_catalog_engine::CatalogEngineError,
    > {
        egret_catalog_engine::search_tabulars(
            self.engine_state.clone(),
            &self.system_user,
            wh,
            query,
        )
        .await
        .map(|v| {
            v.into_iter()
                .map(|r| egret_catalog_engine::BrowseSearchResult {
                    namespace: r.namespace,
                    name: r.name,
                    kind: r.kind.to_string(),
                })
                .collect()
        })
    }

    async fn tasks(
        &self,
        wh: &str,
    ) -> Result<Vec<egret_catalog_engine::BrowseTask>, egret_catalog_engine::CatalogEngineError>
    {
        egret_catalog_engine::list_tasks(self.engine_state.clone(), &self.system_user, wh)
            .await
            .map(|v| {
                v.into_iter()
                    .map(|t| egret_catalog_engine::BrowseTask {
                        queue: t.queue,
                        entity: t.entity,
                        status: t.status,
                    })
                    .collect()
            })
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
    // separate permission vocabulary owned by egret-workspace (dataset/chart/app/…).
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

    // Build the audit sink + spawn the batch writer and retention prune (best-effort).
    // `audit_sink` and `audit_store` are kept in scope after this block: the
    // mutation-emitting states (incl. AuthState's impersonation endpoints) and the
    // query API's CpState clone them.
    let audit_store = egret_audit::AuditStore::new(db.clone());
    // Decision O3: whether governance events follow `audit.enabled` is decided in
    // `egret_audit::governance::records_governance_changes` (today: they never do).
    egret_audit::governance::configure(config.audit.enabled);
    let (audit_sink, audit_rx) = egret_audit::AuditSink::new(config.audit.channel_capacity);
    if config.audit.enabled {
        let store = audit_store.clone();
        tokio::spawn(egret_audit::AuditWriter::run(
            audit_rx,
            store,
            config.audit.batch_size,
            std::time::Duration::from_millis(config.audit.flush_interval_ms),
        ));
        // The SAME `prune(0)` trap the Data API access log closes below, on the
        // table that matters most: `audit.audit_log` is the authorization-decision
        // record. `AuditStore::prune(0)` deletes everything — `ts < now() - 0
        // days` is every row already written, and that is the correct primitive —
        // but a CONFIG key is not a primitive. `0` is what an operator writes
        // when they mean "off", and what `EGRET_AUDIT__RETENTION_DAYS=`
        // deserialises to when it is blank or half-written. Passed straight
        // through, on an hourly tick inside the default-on `audit.enabled`, that
        // empties the audit trail one hour after boot, silently, with nothing
        // having gone wrong and no operator action that looks like a mistake.
        //
        // `egret_dataapi::serving::scheduled_retention` is reused rather than
        // re-implemented here: it is the same decision, and two spellings of it
        // are how one of them ends up wrong. (The dependency direction is
        // acceptable — this binary already composes every seam — but if
        // `egret-audit` ever grows its own scheduler, the guard should move
        // there and both call sites should use that one.)
        //
        // The store keeps its meaning; only the SCHEDULER refuses to call it with
        // a number that would wipe. An operator who genuinely wants an empty
        // table still has `prune(0)`.
        // a-i: an IdP group renamed or removed is seen only at a member's next login,
        // so the OIDC memberships of a member who has not logged in for
        // `OIDC_GROUP_MAX_AGE` are dropped (and their policies recompiled) hourly.
        {
            let groups = egret_auth::groups::GroupRepo::new(db.clone());
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
                loop {
                    tick.tick().await;
                    match groups
                        .drop_stale_oidc_memberships(
                            egret_auth::groups::OIDC_GROUP_MAX_AGE,
                            &egret_perm_compile::PermGroupRecompiler,
                        )
                        .await
                    {
                        Ok(n) if n > 0 => {
                            tracing::info!(dropped = n, "stale OIDC group memberships dropped")
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "stale OIDC group sweep failed"),
                    }
                }
            });
        }
        let configured_audit_retention = config.audit.retention_days;
        if let Some(retention) =
            egret_dataapi::serving::scheduled_retention(configured_audit_retention)
        {
            let prune_store = audit_store.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
                loop {
                    tick.tick().await;
                    if let Err(e) = prune_store.prune(retention).await {
                        tracing::warn!(error=%e, "audit prune failed");
                    }
                }
            });
        } else {
            tracing::warn!(
                retention_days = configured_audit_retention,
                "audit retention is not positive; prune NOT scheduled. \
                 audit.audit_log will grow without bound — set \
                 audit.retention_days to a positive number of days. \
                 (This is deliberately not a wipe: a retention of 0 reads as \
                 'off', never as 'keep nothing'.)"
            );
        }
    }

    // The Data API's access log has its own retention, on the same hourly tick
    // and in the same shape — spec §8. It is spawned HERE, beside the audit
    // prune, rather than near the `DataApiState` construction 700 lines below,
    // because the two are one operational concern: an operator asking "what
    // prunes, and how often" should find both answers in one place.
    //
    // It is NOT gated on `config.audit.enabled`. That flag means "do not record
    // authorization decisions"; the access log is a different table with a
    // different purpose, and reading one switch as governing both would leave
    // the fastest-growing table in the system unpruned for a reason nobody
    // wrote down.
    //
    // A retention of 0 or less means DO NOT PRUNE, and is logged when it is
    // taken. `AccessLogStore::prune(0)` deletes everything — `ts < now() - 0
    // days` is every row already written — so treating a config `0` as "off"
    // (which is what an operator writing `0` means, and what a blank env var
    // deserialises to) is the difference between a knob that is off and a knob
    // that empties the table an hour after boot. The store keeps its meaning;
    // only the scheduler refuses to call it with a number that would wipe.
    {
        let configured = config.data_api.access_log_retention_days;
        if let Some(retention) = egret_dataapi::serving::scheduled_retention(configured) {
            let prune_store = egret_dataapi::serving::AccessLogStore::new(db.clone());
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
                loop {
                    tick.tick().await;
                    match prune_store.prune(retention).await {
                        Ok(n) if n > 0 => {
                            tracing::info!(
                                deleted = n,
                                retention_days = retention,
                                "data-api access log pruned"
                            )
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error=%e, "data-api access log prune failed"),
                    }
                }
            });
        } else {
            tracing::warn!(
                retention_days = configured,
                "data-api access log retention is not positive; prune NOT scheduled. \
                 dataapi.access_log will grow without bound — set \
                 data_api.access_log_retention_days to a positive number of days."
            );
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
    // Build the OIDC service + cookie-encryption key up front (when SSO is
    // enabled): both the auth-state's impersonation step-up (Task 8, which returns
    // an authorize URL + seals the intent into the encrypted state cookie) and the
    // OIDC login/callback router below share the same discovered service and key.
    let (oidc_service, oidc_cookie_key) = if config.oidc.enabled {
        use axum_extra::extract::cookie::Key;
        use base64::Engine;
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(config.security.secret_key.as_bytes())
            .map_err(|e| anyhow::anyhow!("security.secret_key base64: {e}"))?;
        let service = egret_auth::oidc::OidcService::discover(&config.oidc)
            .await
            .map_err(|e| anyhow::anyhow!("oidc discovery: {e}"))?;
        (Some(service), Some(Key::from(&key_bytes)))
    } else {
        (None, None)
    };

    let impersonations = egret_auth::impersonation::ImpersonationRepo::new(db.clone());
    let auth = AuthState {
        users: users.clone(),
        sessions: sessions.clone(),
        config: config.clone(),
        rate_limiter,
        audit: audit_sink.clone(),
        impersonations: impersonations.clone(),
        oidc: oidc_service.clone(),
        cookie_key: oidc_cookie_key.clone(),
    };

    // Shared secret store: one `SecretStore` built from `db` + the master key,
    // reused by the per-workspace storage layer and the database-connection
    // secrets. `SecretStore` is cheap + `Clone`.
    let secrets_crypto = {
        use base64::Engine as _;
        if config.secrets.master_key.is_empty() {
            None
        } else {
            let key = base64::engine::general_purpose::STANDARD
                .decode(config.secrets.master_key.as_bytes())
                .map_err(|e| anyhow::anyhow!("secrets.master_key base64: {e}"))?;
            Some(egret_secrets::Crypto::new(&key).map_err(|e| anyhow::anyhow!(e.to_string()))?)
        }
    };
    let secrets = egret_secrets::SecretStore::new(db.clone(), secrets_crypto.clone());

    // P1b: eagerly upgrade existing storage + DB connection credential secrets from
    // Opaque to `kubernetes.io/basic-auth` (idempotent one-shot, no-op once converged).
    backfill::backfill_basic_auth_credentials(&db, &secrets)
        .await
        .context("P1b basic-auth backfill")?;

    // Move any cleartext webhook signing secret into the vault. Without this the
    // runner — which now resolves `secret_id` only — would silently start
    // delivering those webhooks unsigned.
    backfill::backfill_webhook_secrets(&db, &secrets)
        .await
        .context("automation webhook-secret backfill")?;

    // Epic B: the named per-workspace secret layer over the vault. Built once from
    // the shared vault and threaded into the browse + database states so storage
    // and database connections reference named secrets.
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());

    let oidc = match (oidc_service.clone(), oidc_cookie_key.clone()) {
        (Some(service), Some(cookie_key)) => Some(egret_auth::oidc_routes::OidcRoutesState {
            oidc: service,
            users: users.clone(),
            groups: GroupRepo::new(db.clone()),
            sessions: SessionService::new(
                db.clone(),
                Duration::hours(config.session.absolute_ttl_hours as i64),
                Duration::minutes(config.session.idle_ttl_minutes as i64),
            ),
            config: config.clone(),
            cookie_key,
            impersonations: impersonations.clone(),
            audit: audit_sink.clone(),
            // J-F3 Task 1b: every login's group sync recompiles the policies
            // the groups it moved reach (`egret_auth::groups`' module doc).
            group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
            // KI-185: a sign-in with a verified email redeems the workspace
            // pre-registrations made for it.
            invites: std::sync::Arc::new(MemberRepo::new(db.clone())),
        }),
        _ => None,
    };

    let ranger = if config.ranger.enabled {
        let store = egret_policy::PolicyStore::new(db.clone());
        egret_policy::seed_trino(&store)
            .await
            .map_err(anyhow::Error::from)?;
        // Seed the tag service-def too (Access P3.2) so tag-based policies +
        // `download_tags` have a service-def. LIVE-ONLY field correctness; see
        // `egret_policy::seed_tag`.
        egret_policy::seed_tag(&store)
            .await
            .map_err(anyhow::Error::from)?;
        // Seed the StarRocks service-def too, so StarRocks policy profiles +
        // /service downloads have a def. Resource-based only (no tag service —
        // StarRocks' Ranger plugin does not evaluate tags).
        egret_policy::seed_starrocks(&store)
            .await
            .map_err(anyhow::Error::from)?;
        let mode = match config.ranger.auth_mode {
            egret_config::RangerAuthMode::Required => egret_policy::ranger::AuthMode::Required,
            egret_config::RangerAuthMode::Anonymous => {
                tracing::warn!(
                    "ranger.auth_mode = anonymous: the /ranger plugin surface serves every \
                     caller without a credential, so any engine can read any profile's policies"
                );
                egret_policy::ranger::AuthMode::Anonymous
            }
        };
        Some(egret_policy::RangerState {
            store,
            auth_token: config.ranger.auth_token.clone().into(),
            mode,
            // Engine plugins authenticate with their per-engine credential
            // (current or previous, `egret_engine_mgmt::ranger_credential`),
            // read from the same engine rows and secret store the engine-mgmt
            // seam writes them to.
            verifier: Some(std::sync::Arc::new(
                egret_engine_mgmt::ranger_credential::DbVerifier::new(
                    egret_engine_mgmt::ranger_credential::RangerCredentialStore::new(
                        egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
                        workspace_secrets.clone(),
                    ),
                ),
            )),
        })
    } else {
        None
    };

    // event bus — RoleRepo publishes RolesChanged; the Cedar PDP subscriber is wired in B1b-1
    let bus = egret_events::EventBus::new();

    // Cedar PDP — built from the SAME bus as RoleRepo so RolesChanged hot-reloads
    // it (B1b-0 F1). A different EventBus would silently never reload. Defined
    // above all seam-state constructions so `pdp.clone()` is in scope everywhere.
    // When auditing is enabled, wrap it so every authorize also emits a Decision
    // event (observation only — parity unchanged).
    let cedar = std::sync::Arc::new(egret_auth::pdp::CedarPdp::build(db.clone(), &bus).await?);
    let pdp: std::sync::Arc<dyn egret_core::authz::Pdp> = if config.audit.enabled {
        std::sync::Arc::new(egret_auth::pdp::AuditingPdp::new(cedar, audit_sink.clone()))
    } else {
        cedar
    };

    let workspace = WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: MemberRepo::new(db.clone()),
        access_views: egret_workspace::access_views::AccessViewsRepo::new(db.clone()),
        access_requests: egret_workspace::access_requests::AccessRequestRepo::new(
            db.clone(),
            MemberRepo::new(db.clone()),
        ),
        authz: Authorizer::new(db.clone()),
        pdp: pdp.clone(),
        users: users.clone(),
        sessions: sessions.clone(),
        audit: audit_sink.clone(),
        impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
        oidc: oidc_service.clone(),
        cookie_key: oidc_cookie_key.clone(),
        session_secure: config.session.secure,
    };
    // `Secure` cookies are named `__Host-egret_*` and ONLY those names are read, so a
    // cookie tossed from a sibling host cannot stand in for them (spec §6.1).
    egret_auth::cookie::set_secure_cookies(config.session.secure);

    // `secrets` (the SecretStore) is shared: the per-workspace storage layer and
    // the database block reuse the same instance for their secrets.
    let databases =
        egret_query::DatabaseRepo::new(db.clone(), secrets.clone(), workspace_secrets.clone());

    let database = Some(egret_query::DatabaseState {
        repo: databases.clone(),
        authz: Authorizer::new(db.clone()),
        pdp: pdp.clone(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        workspace_secrets: workspace_secrets.clone(),
    });

    // Who egret IS, and where it is DIALLED from — three answers derived
    // together in `EgretIdentity` (which carries the reasoning and the tests),
    // so none of them can drift from the others:
    //
    // * `issuer()` — the `iss` of every token this deployment signs and the
    //   `issuer` of its discovery document, so it must be a name a client
    //   outside the cluster can resolve.
    // * `in_cluster_base_url()` — the Service address an ENGINE POD resolves for
    //   the JWKS and the catalog. Stays in-cluster even when the issuer is an
    //   ingress hostname: the keys are identical either way, and hairpinning the
    //   data plane's key fetches out through the ingress and back would make
    //   every query depend on the ingress being healthy.
    // * `consumer_base_url()` — where an outside Data API consumer is sent.
    //   Defaults to the ISSUER, not the in-cluster address, so a deployment that
    //   sets only `EGRET_IDP__ISSUER` cannot advertise an externally-valid `iss`
    //   beside an in-cluster `token_endpoint` — a document that cannot be used.
    //
    // Bound once here, before the minter below (its first consumer).
    let identity = egret_bin::identity::EgretIdentity::from_config(&config);
    let dataapi_consumer_url = identity.consumer_base_url().to_string();

    // Package 4 — decide what backs the Data API's rate-limit buckets and
    // response cache, BEFORE anything expensive is built.
    //
    // Placed this early on purpose: the interim guard is a startup REFUSAL, and
    // a refusal that fires after the pool, the PDP and every seam have been
    // constructed is one an operator meets thirty seconds into a rollout instead
    // of immediately. `?` on the error string makes it a hard exit — the process
    // does not come up with silently-multiplied rate limits.
    let (dataapi_cache, dataapi_rate_limits): (
        std::sync::Arc<dyn egret_dataapi::serving::ResponseStore>,
        std::sync::Arc<dyn egret_dataapi::serving::RateLimitStore>,
    ) = match egret_dataapi::serving::choose_store(
        config.data_api.replicas,
        config.data_api.shared_store_url.as_deref(),
    )
    .map_err(|msg| anyhow::anyhow!(msg))?
    {
        egret_dataapi::serving::StoreChoice::InProcess => {
            tracing::info!(
                "data-api serving state is process-local (one replica, no shared store)"
            );
            (
                std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
                std::sync::Arc::new(egret_dataapi::serving::TokenBucket::new()),
            )
        }
        // ONE store behind BOTH seams, so a deployment cannot end up with a
        // shared limiter and a process-local cache by configuring half of it,
        // and so each replica holds one connection rather than two.
        //
        // A connect failure is fatal for the same reason the replica guard is:
        // an operator who configured a shared store and cannot reach it must not
        // be quietly downgraded to the state that multiplies every limit.
        egret_dataapi::serving::StoreChoice::Shared(url) => {
            let store =
                std::sync::Arc::new(egret_dataapi::serving::SharedStore::connect(&url).await?);
            tracing::info!("data-api serving state is shared across replicas");
            (store.clone(), store)
        }
    };

    // Task 4 (STS cutover): the machine-IdP mints the short-lived per-user
    // engine JWT — replacing the old Keycloak refresh-grant broker (deleted;
    // couldn't mint for local users or when OIDC is disabled). Reused by
    // EngineState (B3a SQL Lab delegation) and BiState (B3a per-user
    // delegation) — one shared Arc.
    let engine_minter: std::sync::Arc<dyn egret_analytics_api::engine_token::EngineTokenMinter> =
        std::sync::Arc::new(egret_bin::engine_minter::MachineIdpEngineMinter::new(
            egret_machine_idp::PgSigningKeyStore::new(db.clone()),
            egret_auth::users::UserRepo::new(db.clone()),
            egret_auth::groups::GroupRepo::new(db.clone()),
            // The `iss` of every engine token — identity, not address.
            identity.issuer().to_string(),
            900, // 15-min engine-token TTL (revocation granularity — see spec §7)
        ));

    // Bound before it is wrapped in the `Option` `AppState` wants, because the
    // Data API seam needs the same value as an `Arc<dyn QueryExecPort>` (see the
    // `dataapi` block below) and there must be exactly one `EngineState` behind
    // both — a second one built from the same repos would work today and drift
    // the first time either grows state that matters.
    let engine_state = {
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
        let store = egret_query::QueryStore::new(db.clone());
        let limits = egret_query::execution::model::Limits {
            max_rows: config.engine.max_rows,
            timeout: std::time::Duration::from_secs(config.engine.query_timeout_secs),
        };
        let bucket_uri = format!("s3://{}", config.storage.bucket);
        let jobs = egret_query::JobManager::new(
            store.clone(),
            databases.clone(),
            std::sync::Arc::new(egret_query::execution::connector::DefaultConnectorFactory),
            storage.clone(),
            bucket_uri.clone(),
            config.engine.results_prefix.clone(),
            limits.clone(),
            config.engine.preview_rows,
        );
        egret_query::EngineState {
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
            pdp: pdp.clone(),
            users: UserRepo::new(db.clone()),
            engine_minter: engine_minter.clone(),
        }
    };
    let engine = Some(engine_state.clone());

    // W9a: the workflow engine's execution files live in the SAME object store
    // (this `S3Store`, the `[storage]` bucket -- one client, not a second one),
    // under `workflow.storage.prefix`. Installed process-wide BEFORE the
    // workflow runtime and router below, because `read()` runs inside `settle`,
    // which none of its eight entry points can hand a store to
    // (`egret_workflow::storage`'s module doc). Index lookups never go through
    // it: they run on the caller's connection.
    if egret_workflow::storage::install(std::sync::Arc::new(egret_workflow::storage::FileStore {
        store: engine_state.storage.clone(),
        bucket: config.storage.bucket.clone(),
        prefix: config.workflow.storage.prefix.clone(),
        caps: egret_workflow::storage::Caps::from(&config.workflow.storage),
        retention_days: config.workflow.storage_retention_days,
    }))
    .is_err()
    {
        anyhow::bail!("the workflow file store was installed twice");
    }
    // KI-112: a workflow's SECRET inputs are sealed with the secret store's own
    // key, installed process-wide for the same reason as the file store: the
    // render that opens them runs inside `settle`. Without a key a start that
    // would store a SECRET value is refused. Then the executions stored before
    // migration 0203 get their clear-text SECRET values masked (and sealed).
    if let Some(crypto) = secrets_crypto {
        if !egret_workflow::secret_inputs::install(crypto) {
            anyhow::bail!("the workflow SECRET-input cipher was installed twice");
        }
    }
    let masked = egret_workflow::secret_inputs::backfill(&db.pool)
        .await
        .map_err(|e| anyhow::anyhow!("masking stored SECRET inputs: {e}"))?;
    if masked > 0 {
        tracing::info!(executions = masked, "masked SECRET inputs stored in clear");
    }
    // no-silent-stall §4.1: the task types this deployment's worker runs,
    // from the same configuration `handlers_for` builds the worker's handlers
    // from below (a unit test holds the two together). Installed process-wide
    // BEFORE the router and the worker start: `spec::unrunnable` judges a flow
    // against it, and `runtime::served()` panics rather than defaulting to
    // "serve everything" in a process that never installed one.
    if egret_workflow::runtime::install_served(egret_workflow::runtime::served_for(
        &config.workflow,
    ))
    .is_err()
    {
        anyhow::bail!("the workflow's served task types were installed twice");
    }
    // W9b: namespace-file lookups at render time (`fileURI`, `read` of a
    // namespace file) run on their own connection, made from this pool's
    // options, for the reason above: a render's entry points hold no pool.
    if egret_workflow::storage::nsfiles::install_db((*db.pool.connect_options()).clone()).is_err() {
        anyhow::bail!("the namespace file database options were installed twice");
    }

    let bi = Some(egret_bi::BiState {
        db: db.clone(),
        saved: egret_bi::SavedQueryRepo::new(db.clone()),
        schema: egret_bi::SchemaService::new(),
        databases: databases.clone(),
        users: UserRepo::new(db.clone()),
        engine_minter: engine_minter.clone(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        pdp: pdp.clone(),
        charts: egret_bi::ChartRepo::new(db.clone()),
        datasets: egret_bi::DatasetRepo::new(db.clone()),
        dashboards: egret_bi::DashboardRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db.clone()),
        audit: audit_sink.clone(),
    });

    // The tick engine (P2), built unconditionally and shared: `AutomationState`
    // holds the SAME `Arc` the ticker below spawns (when
    // `config.automation.enabled`), so P5's "Run now" route runs a schedule
    // through the exact `evaluate_and_deliver` the tick uses rather than a
    // second instance that could quietly drift from it. Building it here does
    // not start anything — only `runner::spawn`, further down, does that.
    let automation_runner = std::sync::Arc::new(egret_automation::runner::AutomationRunner {
        db: db.clone(),
        schedules: egret_automation::ScheduleRepo::new(db.clone(), secrets.clone()),
        databases: databases.clone(),
        users: UserRepo::new(db.clone()),
        minter: engine_minter.clone(),
        pdp: pdp.clone(),
        dataapi_endpoints: std::sync::Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
        query: std::sync::Arc::new(egret_automation::evaluator::EngineRunner {
            databases: databases.clone(),
            factory: std::sync::Arc::new(
                egret_query::execution::connector::DefaultConnectorFactory,
            ),
            limits: egret_query::execution::model::Limits {
                max_rows: config.engine.max_rows,
                timeout: std::time::Duration::from_secs(config.engine.query_timeout_secs),
            },
        }),
        resolver: std::sync::Arc::new(egret_automation::deliver::destination::SystemResolver),
        policy: egret_automation::deliver::destination::SsrfPolicy {
            allow_hosts: config.automation.webhook_allow_hosts.clone(),
        },
        smtp: egret_automation::deliver::smtp_config::SmtpConfigRepo::new(
            db.clone(),
            secrets.clone(),
        ),
        slack_config: egret_automation::deliver::slack_config::SlackConfigRepo::new(
            db.clone(),
            secrets.clone(),
        ),
        charts: egret_bi::ChartRepo::new(db.clone()),
        dashboards: egret_bi::DashboardRepo::new(db.clone()),
        datasets: egret_bi::DatasetRepo::new(db.clone()),
        render_tokens: egret_automation::render::token::RenderTokenRepo::new(db.clone()),
        renderer: std::sync::Arc::new(egret_automation::render::client::HttpRenderer::new(
            config.automation.renderer_url.clone(),
        )),
        public_url: config.automation.public_url.clone(),
        // Falls back to `public_url` when unset: in local dev the renderer's
        // address and the browser's really are the same host.
        browser_url: config
            .automation
            .browser_url
            .clone()
            .unwrap_or_else(|| config.automation.public_url.clone()),
    });

    let automation = Some(egret_automation::AutomationState {
        db: db.clone(),
        schedules: egret_automation::ScheduleRepo::new(db.clone(), secrets.clone()),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        pdp: pdp.clone(),
        smtp: egret_automation::deliver::smtp_config::SmtpConfigRepo::new(
            db.clone(),
            secrets.clone(),
        ),
        slack_config: egret_automation::deliver::slack_config::SlackConfigRepo::new(
            db.clone(),
            secrets.clone(),
        ),
        render_tokens: egret_automation::render::token::RenderTokenRepo::new(db.clone()),
        charts: egret_bi::ChartRepo::new(db.clone()),
        dashboards: egret_bi::DashboardRepo::new(db.clone()),
        datasets: egret_bi::DatasetRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db.clone()),
        databases: databases.clone(),
        users: UserRepo::new(db.clone()),
        engine_minter: engine_minter.clone(),
        // Fix-round-2 (Task 5): a fresh `EndpointStore` rather than reusing
        // `dataapi.endpoints` below — it is `Clone` and holds only `db`, so a
        // second instance is free, and building it here keeps this block
        // constructible without caring whether `dataapi` (built further down
        // this function) exists yet.
        dataapi_endpoints: std::sync::Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
        // Same `EngineRunner` shape `automation_runner` above uses for alert
        // evaluation, but with the chart-run ceiling (`egret_bi`'s
        // `MAX_CHART_ROWS`/30s) rather than the alert evaluator's
        // one-row/`config.engine` limits — a rendered report needs every row
        // a chart was built to show, not just its first cell.
        query: std::sync::Arc::new(egret_automation::evaluator::EngineRunner {
            databases: databases.clone(),
            factory: std::sync::Arc::new(
                egret_query::execution::connector::DefaultConnectorFactory,
            ),
            limits: egret_query::execution::model::Limits {
                max_rows: egret_bi::chartquery::MAX_CHART_ROWS as usize,
                timeout: std::time::Duration::from_secs(30),
            },
        }),
        runner: automation_runner.clone(),
    });

    // `reaper_cancel` is the background loops' shared cancellation token: the
    // automation ticker and the workflow runtime below, cancelled together on
    // SIGTERM or SIGINT by the task spawned after the workflow runtime.
    let reaper_cancel = tokio_util::sync::CancellationToken::new();

    // ── Alerts & Reports ticker (P2) ───────────────────────────────────────
    //
    // One per replica, cancelled by the same token as the workflow runtime
    // below. No leader election: the claim uses FOR UPDATE SKIP LOCKED, so N
    // replicas ticking together is the design rather than a hazard.
    //
    // `automation_runner` itself is always built (above, shared with
    // `AutomationState.runner` for P5's "Run now"); this flag only decides
    // whether the periodic claim loop runs — a replica with it off can still
    // serve a manual run.
    if config.automation.enabled {
        egret_automation::runner::spawn(
            automation_runner,
            std::time::Duration::from_secs(config.automation.tick_interval_secs),
            reaper_cancel.clone(),
        );
        tracing::info!(
            interval_secs = config.automation.tick_interval_secs,
            "automation ticker started"
        );
    } else {
        tracing::info!("automation ticker disabled by config");
    }

    // ── Workflow runtime: the claim loop and the lease reaper (W10 Task 2) ──
    //
    // **This is what makes the workflow engine run.** Before it, a webhook or an
    // API call created a `workflow.executions` row and nothing ever advanced it:
    // `egret_workflow::worker::tick` had no caller in any `crates/*/src`, so
    // every durability property W3b and W4 prove was proved about code no
    // deployment ran.
    //
    // Two tickers, one per replica, cancelled by the same token as everything
    // else above. No leader election — but the reason is NOT one sentence
    // inherited four times: the claim's `FOR UPDATE SKIP LOCKED` covers the
    // claim and nothing else, and `egret_workflow::runtime`'s module doc gives
    // a separate safety statement for `wake`, `due_retries` and `reap`, each
    // proved by a test in `crates/egret-workflow/tests/runtime.rs` that runs two
    // tickers against one database.
    //
    // The gate is inside `runtime::spawn` rather than an `if` here, so that
    // exactly one thing decides and a test can reach it; `worker_enabled`
    // defaults to `false` (`egret_config::WorkflowConfig::worker_enabled`).
    //
    // W12 adds a third ticker, the schedule-trigger loop, behind its OWN gate
    // (`scheduler_enabled`, also `false` by default) — independent of the
    // worker's, so the two are passed separately and never derived from each
    // other. Its per-statement safety argument is `egret_workflow::scheduler`'s
    // module doc.
    //
    // **Which handlers run is `egret_workflow::runtime::handlers_for`'s
    // decision (W5a Task 5), made there so a test can reach it.** With
    // `workflow.container_runner = none` (the default) it is
    // `Handlers::with_core_tasks()` -- eleven handlers: `core.log.Log`,
    // `core.debug.Return`, `core.execution.Fail`, `core.flow.Sleep`, since W9a
    // Task 11 `core.storage.Write`/`Size`, and since W9a Task 12
    // `core.storage.Concat`/`Split`/`Reverse`/`DeduplicateItems`/`FilterItems`,
    // the storage ones over the store installed above -- plus
    // `core.storage.Delete`/`PurgeCurrentExecutionFiles`, which need the
    // pool: thirteen of `spec::registry::TASK_TYPES`' twenty-three rows.
    // *(2026-10-28, package #4 Task C6: that count was long stale. Counted
    // now: `with_core_tasks` is thirteen handlers -- the eleven above plus
    // group C1's `OutputValues` and `Assert` -- and `handlers_for` adds
    // seventeen on every deployment (the two storage tasks, four namespace
    // files tasks, six KV tasks, `PurgeLogs`, the three http tasks and
    // `egret.sql`): thirty of the forty rows. The other ten are the four
    // script types below, the four executor-level rows
    // (`spec::registry::EXECUTOR_LEVEL`, no handler on purpose) and the two
    // deferred `egret.*` types; `egret_workflow::runtime`'s
    // `every_row_is_served_executor_level_a_script_or_deferred` holds that
    // partition.)* With `docker` or `kubernetes` it adds Kestra's four script
    // task types and W4's two aliases over ONE runner built here, gated at run
    // time by the workspace feature `workflow_containers` (off by default) and
    // the `plugin_configurations` image allowlist, and the reaper's tick
    // sweeps that runner's orphans. A Kubernetes configuration egret refuses
    // (or no cluster client) stops the process here rather than starting
    // without the containers the operator asked for.
    //
    // `egret.sql` is registered on every deployment since package #4 Task C5,
    // over `query_port` -- the ONE `Arc<EngineState>` the Data API and the
    // Data Apps gateway below share, built here so `handlers_for` can take it
    // (it was built after this call until then, and the comment here said
    // "`egret.sql` is not registered: it needs an `Arc<dyn QueryExecPort>`
    // that nothing here provides").
    //
    // **A flow whose task type has no handler here no longer HANGS** (a
    // `TASK_TYPES` type this deployment does not wire in). Since
    // no-silent-stall Task 2 the set this deployment
    // serves is installed above (`runtime::served_for`, the same keys these
    // handlers are registered under) and `spec::unrunnable` names each task
    // it lacks (`task_type_not_served`). Since Task 3 every validation report
    // carries it, and publish, every start (API, webhook, schedule, Flow
    // trigger, replay) and a `Subflow` target refuse it, so no NEW execution
    // of such a flow is created. What still reaches the worker is an
    // execution started before that refusal (on an older binary, or under a
    // configuration that served the type), and since Task 4 the worker FAILS
    // that run (`WorkerError::NoHandler`, spec §5) with the run-log line "egret
    // cannot run task type `X` on this deployment: no handler is registered
    // for it. This is not an error in the flow.", a `warn` in the server log,
    // and `retry:`/`errors:` applied as for any `FAILED`. Until Task 4 the
    // error propagated out of `run_one`, the run kept its lease, `reap`
    // cleared only `claimed_at` a `working_timeout` later, `claim` re-selected
    // it, and the cycle repeated for ever; `oracle-exec/README.md` item 19
    // records that history. An old execution whose stored source validation
    // now refuses (a type with no `TASK_TYPES` row, an `AllowFailure`, a
    // `WorkingDirectory`, ...) is covered too: the worker plans a stored
    // execution without validation (`executor::plan_stored`; validation gates
    // starts only), so its run reaches `NoHandler` or `NotALeaf` and fails.
    // **Two registered `egret.*` types have no handler, whatever the
    // runner**, and since Task 3 neither publishes nor starts
    // (`egret.dataapi.call`, `egret.report.send`; package #4 spec D10 defers
    // them). This said "Five registered `egret.*`/short `core.*` types" and
    // also listed `egret.dataset.refresh`, `core.http` and `core.slack` until
    // package #4 Task C6 (2026-10-28) removed those three rows: a flow naming
    // one is now `unknown_task_type`. `egret.sql` was the sixth until package #4
    // Task C5 (2026-10-28) registered it; this said "Six of twenty-three ...
    // 23 rows = these 6 + the 13 core handlers above + the 4 script types a
    // runner serves" until then, and the row count was already stale (see
    // `spec::registry::TASK_TYPES`). All nine
    // core storage tasks are registered -- W9a Task 11's `Write` and `Size`
    // and Task 12's five in `with_core_tasks`, `Delete` and
    // `PurgeCurrentExecutionFiles` by `handlers::storage::register` inside
    // `handlers_for`. This comment said "seven of twenty-four" until the W9a rebase onto Data
    // Apps P6a (#629), which removed `egret.python`; "seven of nineteen" until W9a Task 12
    // fix round 1, "seven of fifteen" until W9a Task 11, and "nine of
    // thirteen" before W5a folded `egret.container`/`egret.script.python`
    // into aliases of four script types that a runner now serves. Publish
    // (`publish::check_script_tasks`) refuses EVERY script task when no
    // runner is configured -- with or without a `taskRunner:` -- and one whose
    // `taskRunner:` names a runner other than the configured one. With
    // `none`, since Task 3, every start refuses a script task too (a draft
    // run included), so one reaches that loop only from an execution started
    // under an earlier configuration, which this binary cannot refuse.
    //
    // W11: `spawn_with_logs` takes the `Handlers` unshared so it can give them
    // the Postgres log sink (`Handlers::with_log_sink`) before wrapping them;
    // WHICH handlers run is still decided here. With the worker on it also
    // starts the log writer and the retention pruner. The writer is stopped by
    // the claim loop's end, not by the token, so a tick in flight finishes
    // writing before the writer drains; `workflow_runtime.join()` below waits
    // for that at shutdown.
    //
    // ONE `Arc` around the query engine, cloned into every seam that reaches it
    // as a `QueryExecPort` (`egret.sql` here, the Data API and the Data Apps
    // gateway below), so there is exactly one `EngineState` behind all three.
    let query_port = std::sync::Arc::new(engine_state);
    let (workflow_handlers, container_runner) = egret_workflow::runtime::handlers_for(
        &config.workflow,
        db.pool.clone(),
        // W5b: `secret()` on `kestra.*` tasks reads the execution's
        // workspace's secrets, never a global one.
        std::sync::Arc::new(egret_bin::workflow_secrets::WorkspaceSecretSource::new(
            workspace_secrets.clone(),
        )),
        // Package #4 Task C5: `egret.sql`'s port.
        query_port.clone(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("workflow.container_runner: {e}"))?;
    let workflow_runtime = egret_workflow::runtime::spawn_with_logs(
        db.pool.clone(),
        workflow_handlers,
        egret_workflow::RuntimeConfig {
            enabled: config.workflow.worker_enabled,
            tick_interval: std::time::Duration::from_secs(config.workflow.tick_interval_secs),
            batch: config.workflow.batch,
            max_concurrent_runs: config.workflow.max_concurrent_runs,
            reaper_interval: std::time::Duration::from_secs(config.workflow.reaper_interval_secs),
            working_timeout: std::time::Duration::from_secs(config.workflow.working_timeout_s),
            container_default_timeout: std::time::Duration::from_secs(
                config.workflow.container_default_timeout_s,
            ),
            kill_poll: std::time::Duration::from_millis(config.workflow.kill_poll_ms),
            scheduler_enabled: config.workflow.scheduler_enabled,
            trigger_interval: std::time::Duration::from_secs(config.workflow.trigger_interval_secs),
            log_limits: egret_workflow::logs::LogLimits {
                run_max_lines: config.workflow.log_run_max_lines,
                run_max_bytes: config.workflow.log_run_max_bytes,
            },
            // Saturated at i32::MAX days. NOT "for ever": above about 2.4
            // million days `now() - make_interval(days => n)` is out of
            // Postgres's timestamp range, and every prune pass fails (and is
            // logged). `0` is the way to keep lines for ever.
            log_retention_days: i32::try_from(config.workflow.log_retention_days)
                .unwrap_or(i32::MAX),
        },
        reaper_cancel.clone(),
    );
    // W9b Task 8 fix round 1: the hourly sweep of namespace-file staging
    // objects a crash left behind (older than 24 h). On every replica with a
    // store, worker or not: the namespace file API uploads through staging
    // wherever it is served, and two sweeps of one row are no-ops.
    if let Some(fs) = egret_workflow::storage::current() {
        egret_workflow::storage::nsfiles::staging::spawn_staging_sweeper(
            db.pool.clone(),
            fs.clone(),
            egret_workflow::storage::nsfiles::staging::StagingSweepConfig::default(),
            reaper_cancel.clone(),
        );
        // Workflow debt D6: the hourly sweep of the `_wd/{block}/` snapshots
        // a crash, a forced kill or a failed discard left, 24 h after the
        // block ended. On every replica with a store: a pass takes an
        // advisory lock, so one sweep runs per database at a time.
        egret_workflow::storage::wd_sweep::spawn_wd_sweeper(
            db.pool.clone(),
            fs.clone(),
            egret_workflow::storage::wd_sweep::WdSweepConfig::default(),
            reaper_cancel.clone(),
        );
        // W9c spec §11: the hourly KV expiry sweep, its first pass an hour
        // after start, over every workspace. On every replica with a store:
        // each key is try-locked, so two replicas never sweep one key twice.
        egret_workflow::storage::kv::sweep::spawn_kv_expiry_sweeper(
            db.pool.clone(),
            fs.clone(),
            Default::default(),
            reaper_cancel.clone(),
        );
    }
    // SIGTERM or SIGINT (`shutdown_signal` observes both) stops every
    // background loop on `reaper_cancel` -- the automation ticker and the
    // workflow runtime -- at the signal, while the HTTP server drains.
    // (Before W11 only Ctrl-C cancelled it, from a task of
    // its own; on SIGTERM the loops ran on until `main` returned.) So the
    // workflow claim loop stops claiming and its log writer drains inside the
    // grace period, and other replicas pick up the runs.
    {
        let reaper_cancel = reaper_cancel.clone();
        tokio::spawn(async move {
            egret_server::shutdown_signal().await;
            tracing::info!("shutdown signal received, stopping background loops");
            reaper_cancel.cancel();
        });
    }

    // Embedded catalog (EB-1/EB-3): mount lakekeeper in-process at /catalog/v1
    // (fronted by egret's CatalogGuard), and build the warehouse provisioner —
    // both need the lakekeeper ApiContext built here.
    let mut catalog_provisioner: Option<
        std::sync::Arc<dyn egret_catalog_engine::WarehouseProvisioner>,
    > = None;
    let mut catalog_browser: Option<std::sync::Arc<dyn egret_catalog_engine::WarehouseBrowser>> =
        None;
    // View governance B3: the lineage sweep, run on a schedule here and on
    // demand from the classification page. With no embedded catalog there is
    // nothing to sweep, and the route says so.
    let mut lineage_sweeper: std::sync::Arc<dyn egret_catalog_engine::LineageSweeper> =
        std::sync::Arc::new(egret_catalog_engine::NoCatalogSweeper);
    // View governance B5: the DEFINER audit reads the views of egret's
    // catalogs. With no embedded catalog there are none.
    let mut view_catalog: std::sync::Arc<dyn egret_catalog_engine::ViewCatalog> =
        std::sync::Arc::new(egret_catalog_engine::NoViewCatalog);
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
        let table_hook = std::sync::Arc::new(egret_catalog_engine::TableHook::new(db.clone()));
        catalog_browser = Some(std::sync::Arc::new(LakekeeperBrowser {
            engine_state: engine_state.clone(),
            system_user: config.catalog.system_user.clone(),
            table_hook: table_hook.clone(),
        }));
        let guard: std::sync::Arc<dyn egret_catalog_engine::CatalogGuard> =
            std::sync::Arc::new(EgretCatalogGuard {
                keys: egret_machine_idp::PgSigningKeyStore::new(db.clone()),
                catalog: egret_catalog_engine::CatalogStore::new(db.clone()),
            });
        // View governance B2: a classified view's commit waits until every
        // Ranger plugin holds its tags. The lineage EXPLAIN runs as
        // `egret-lineage` on a provisioned JWT-mode Trino engine
        // (`egret_query::execution::lineage`), with a token only egret mints.
        let lineage_minter =
            std::sync::Arc::new(egret_bin::engine_minter::MachineIdpEngineMinter::new(
                egret_machine_idp::PgSigningKeyStore::new(db.clone()),
                egret_auth::users::UserRepo::new(db.clone()),
                egret_auth::groups::GroupRepo::new(db.clone()),
                identity.issuer().to_string(),
                900,
            ));
        let lineage_runner =
            std::sync::Arc::new(egret_query::execution::lineage::TrinoLineageRunner::new(
                db.pool.clone(),
                lineage_minter,
                // The hold bounds the EXPLAIN by the same budget, inside its
                // one 90 s deadline (`egret_catalog_engine::view_hold`).
                egret_catalog_engine::EXPLAIN_BUDGET,
            ));
        let view_hold = std::sync::Arc::new(egret_catalog_engine::ViewHold::new(
            db.clone(),
            lineage_runner.clone(),
        ));
        let sweep = std::sync::Arc::new(egret_catalog_engine::LineageSweep::new(
            db.clone(),
            engine_state.clone(),
            config.catalog.system_user.clone(),
            lineage_runner,
        ));
        lineage_sweeper = sweep.clone();
        view_catalog = std::sync::Arc::new(egret_catalog_engine::EmbeddedViewCatalog::new(
            db.clone(),
            engine_state.clone(),
            config.catalog.system_user.clone(),
        ));
        // Task 9 fix round 1: after each classification write commits, sweep
        // the views with no computed lineage that its profiles reach.
        egret_catalog_engine::spawn_classification_sweeps(sweep.clone(), db.clone());
        // The scheduled sweep (spec B3, R6). The first run waits one period,
        // so a boot does not EXPLAIN every view before the engines are up. A
        // run that overlaps an on-demand one is skipped (`SweepError::Busy`).
        tokio::spawn(async move {
            use egret_catalog_engine::LineageSweeper as _;
            let period = egret_catalog_engine::LINEAGE_SWEEP_INTERVAL;
            let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            // M6: a sweep longer than the period does not trigger a burst.
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                match sweep.sweep().await {
                    Ok(report) if !report.failures.is_empty() => tracing::warn!(
                        failures = report.failures.len(),
                        first = ?report.failures.first(),
                        "lineage sweep: some views' lineage could not be computed; their tags \
                         are unchanged"
                    ),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "lineage sweep did not run"),
                }
            }
        });
        Some(egret_catalog_engine::mount_catalog_engine(
            engine_state,
            Some(guard),
            Some(view_hold),
            Some(table_hook),
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
    //
    // `[engines] provisioner` picks the backend (`select_provisioner`). `auto`
    // is exactly the above; `docker` never builds a kube client.
    let kube: Option<std::sync::Arc<dyn egret_provisioner::KubeClient>> = if !matches!(
        config.engines.provisioner,
        egret_config::Provisioner::Auto | egret_config::Provisioner::Kubernetes
    ) {
        None
    } else {
        let kube = match egret_provisioner::LiveKube::in_cluster().await {
            Ok(k) => Some(k),
            Err(_) => egret_provisioner::LiveKube::from_kubeconfig().await.ok(),
        };
        match kube {
            Some(k) => Some(std::sync::Arc::new(k)),
            None => {
                tracing::warn!("kubernetes unavailable; engine provisioning disabled");
                None
            }
        }
    };
    let selection =
        select_provisioner(&config.engines, kube.is_some()).map_err(|e| anyhow::anyhow!(e))?;
    let mut coordinator_transport: Option<
        std::sync::Arc<dyn egret_engine_mgmt_api::CoordinatorTransport>,
    > = None;
    let engine_provisioner: Option<
        std::sync::Arc<dyn egret_engine_mgmt_api::EngineProvisioningPort>,
    > = match selection {
        Selection::Kubernetes => kube.clone().map(|kube| {
            let trino = std::sync::Arc::new(egret_provisioner::TrinoProvisioner::new(kube.clone()));
            let starrocks =
                std::sync::Arc::new(egret_provisioner::StarRocksProvisioner::new(kube.clone()));
            std::sync::Arc::new(EgretEngineProvisioningPort::new(trino, starrocks, kube))
                as std::sync::Arc<dyn egret_engine_mgmt_api::EngineProvisioningPort>
        }),
        Selection::Docker => {
            let d = &config.engines.docker;
            let api = egret_provisioner::docker::BollardDocker::connect(&d.host)
                .map_err(|e| anyhow::anyhow!(e))?;
            api.ping()
                .await
                .map_err(|e| anyhow::anyhow!("docker daemon at {} is unreachable: {e}", d.host))?;
            tracing::info!(host = %d.host, network = %d.network, "engine provisioning: docker");
            coordinator_transport = Some(std::sync::Arc::new(
                egret_provisioner::docker::HttpCoordinatorTransport::new(),
            ));
            Some(
                std::sync::Arc::new(egret_provisioner::docker::DockerEngineProvisioner::new(
                    std::sync::Arc::new(api),
                    d.network.clone(),
                    d.project.clone(),
                    d.allowed_images.clone(),
                ))
                    as std::sync::Arc<dyn egret_engine_mgmt_api::EngineProvisioningPort>,
            )
        }
        Selection::Disabled => None,
    };

    // In-cluster URL Trino pods use to reach egret's REST catalog + token
    // endpoint; shared by the engine-mgmt seam (provisioning) and control-plane
    // (policies' `trino_config` route renders the same base URL).
    let engine_egret_url = match (&selection, &config.engines.docker.callback_url) {
        (Selection::Docker, Some(url)) => url.clone(),
        _ => config
            .catalog
            .public_url
            .clone()
            .unwrap_or_else(|| "http://egret:8080".to_string()),
    };

    // Recompiles a catalog's implicit assignment grant (SP4a Task 16, D3
    // defects 2 and 3) — built once here, shared (as two DIFFERENT trait
    // objects, one per port) with the identical instance `perm_compiler`
    // below constructs for engine-mgmt. `PermStore`/`PolicyStore` are cheap
    // pool handles, so cloning the concrete struct instead of the `Arc` would
    // cost nothing extra either; sharing the `Arc` is simplest.
    let profile_compiler = std::sync::Arc::new(ProfileCompilerImpl {
        perm: egret_perm::PermStore::new(db.pool.clone()),
        policy: egret_policy::PolicyStore::new(db.clone()),
    });

    // Catalog seam admin router (E3d-3): warehouses / catalog-roles / grants /
    // Catalog Clients / warehouse browse+admin. Moved out of egret-controlplane
    // into egret-catalog-engine; still nested at `/api/control-plane`. The
    // warehouse provisioner (built above) creates lakekeeper warehouses.
    let catalog_admin = Some(egret_catalog_engine::CatalogAdminState {
        store: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        secrets: workspace_secrets.clone(),
        provisioner: catalog_provisioner,
        browser: catalog_browser.clone(),
        sessions: sessions.clone(),
        pdp: pdp.clone(),
        perm_compiler: Some(profile_compiler.clone()
            as std::sync::Arc<dyn egret_catalog_registry::CatalogAssignmentCompiler>),
    });

    // Policy/Ranger seam admin router (E2d): the Policies console. Owns the
    // policy store; reads the engine seam through `EngineBindingReader` (which
    // engines bind to which profile) — wired here in the composition root.
    let policy_admin = Some(egret_policy::PolicyAdminState {
        store: egret_policy::PolicyStore::new(db.clone()),
        engines: std::sync::Arc::new(EngineBindingReaderImpl {
            store: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
            catalog: config
                .catalog
                .enabled
                .then(|| egret_catalog_engine::CatalogStore::new(db.clone())),
            browser: catalog_browser.clone(),
        }),
        users: egret_auth::users::UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        sessions: sessions.clone(),
        engine_egret_url: engine_egret_url.clone(),
        pdp: pdp.clone(),
        // Final review I-3: an editor write to a StarRocks data mask refolds the
        // clearance masks in the same transaction.
        recompiler: std::sync::Arc::new(egret_perm_compile::PermRecompiler),
    });

    // Data-permission model surface (SP4a Task 9): grants/rules/explain, plus
    // the manual compile trigger. Both stores are built over the SAME `db`,
    // which is what lets a grant write and its recompile share one
    // transaction across the `perm.*` and `policy.*` schemas.
    let perm = Some(egret_perm_compile::PermState {
        perm: egret_perm::PermStore::new(db.pool.clone()),
        policy: egret_policy::PolicyStore::new(db.clone()),
        sessions: sessions.clone(),
        pdp: pdp.clone(),
    });
    // J-F3 Task 2 (design R5): one `warn!` per stored grant principal or rule
    // exemption whose ref matches nobody where that fails OPEN. Off the boot
    // path and best-effort: the rows are served either way, and
    // `GET /perm/unresolved-principals` is where an operator acts on them.
    {
        let pool = db.pool.clone();
        tokio::spawn(async move {
            if let Err(e) =
                egret_perm_compile::principal_refs::warn_fail_open_principals(&pool).await
            {
                tracing::warn!(error = %e, "the stored principal-ref check did not run");
            }
        });
    }
    // Design R7 (2026-10-18): recompile every profile whose model names a
    // role, so a body an older compile wrote without a holder who had no
    // username names them now (role expansion mints since R5). Idempotent and
    // best-effort: an unchanged profile writes nothing, and a failure is
    // logged and never blocks boot.
    {
        let pool = db.pool.clone();
        tokio::spawn(async move {
            if let Err(e) =
                egret_perm_compile::role_membership::recompile_role_referencing_profiles(&pool)
                    .await
            {
                tracing::warn!(
                    error = %e,
                    "the boot recompile of role-referencing profiles did not run"
                );
            }
        });
    }
    // Expiry sweep (narrowing 2 of `egret_perm_compile::members`): a grant that
    // lapses is not a write, so an hourly pass recompiles the profiles it named.
    // Spawned here, off the boot path; the first tick runs at once in the
    // background. Idempotent; a failure is logged and the next run retries.
    {
        let pool = db.pool.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tick.tick().await;
                match egret_perm_compile::expiry::recompile_lapsed_grants(&pool).await {
                    Ok(s) if s.lapsed_rows > 0 => tracing::info!(
                        lapsed_rows = s.lapsed_rows,
                        profiles = s.services.len(),
                        rewritten = s.rewritten.len(),
                        "expiry sweep ran"
                    ),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "expiry sweep failed; retried next run"),
                }
            }
        });
    }
    // Username takeover (spec 2026-10-23 §4.2): converge every `egret-admin`
    // set to the active platform admins, repairing bodies written before the
    // fix. Best-effort; a failure is logged and never blocks boot.
    {
        let pool = db.pool.clone();
        tokio::spawn(async move {
            match egret_perm_compile::sync_admin_policies(&pool).await {
                Ok(n) => tracing::info!(services = n, "egret-admin policies synced at boot"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "the boot sync of the egret-admin policies did not run"
                ),
            }
        });
    }
    // D2: StarRocks whole-table classifications written before this version
    // compiled nothing there; the first boot compiles them, one profile at a
    // time, and every boot retries a table-commit recompile that failed. A
    // refused profile keeps its previous compile and is recorded.
    {
        let pool = db.pool.clone();
        tokio::spawn(async move {
            match egret_perm_compile::recompile_table_classified_profiles(&pool).await {
                Ok(n) => tracing::info!(
                    profiles = n,
                    "StarRocks profiles with whole-table classifications compiled at boot"
                ),
                Err(e) => tracing::warn!(
                    error = %e,
                    "the boot compile of StarRocks whole-table classifications did not run"
                ),
            }
        });
    }
    // Migration 0207 renamed classifications that shared a name; their
    // compiled tag policies still name the old one until this recompiles them
    // (see `TagRepo::recompile_queued_renames`). Off the boot path: until it
    // commits no `tag_version` moves, so plugins keep serving the old map,
    // which agrees with the old policies. A failure is logged and retried at
    // the next boot.
    {
        let tags = egret_bi::TagRepo::new(db.clone());
        tokio::spawn(async move {
            match tags.recompile_queued_renames().await {
                Ok(0) => {}
                Ok(n) => tracing::info!(
                    renamed = n,
                    "recompiled the policies of classifications migration 0207 renamed"
                ),
                Err(e) => tracing::warn!(
                    error = %e,
                    "the recompile after migration 0207's classification renames did not run; \
                     it is retried at the next boot"
                ),
            }
        });
    }
    // Design R5 (2026-10-17): one `warn!` per group membership whose
    // provenance is unknown — local groups with OIDC-provider members (a login
    // before R1 may have added them), and local users in OIDC groups (no login
    // removes them). Report only: no row is changed, there is no endpoint, and
    // a failure is logged and never blocks boot.
    {
        let pool = db.pool.clone();
        tokio::spawn(async move {
            if let Err(e) = egret_auth::groups::warn_unknown_membership_provenance(&pool).await {
                tracing::warn!(error = %e, "the group-membership provenance check did not run");
            }
        });
    }

    // Engine-mgmt seam: owns the engine domain (HTTP router + provisioning).
    let engine_mgmt = Some(egret_engine_mgmt::EngineMgmtState {
        engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
        // Assignment service (P1): materializes/removes the read-only delegated
        // query connection when an engine is assigned to a workspace. Reuses the
        // shared `databases` repo built above.
        assignment: egret_engine_mgmt::EngineAssignmentService::new(
            egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
            databases.clone(),
        ),
        catalog: std::sync::Arc::new(egret_catalog_engine::CatalogStore::new(db.clone())),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        // Policy/Ranger seam behind its `PolicyPort` contract (E2a); the concrete
        // in-process impl is the shared `PolicyStore`.
        authz: std::sync::Arc::new(egret_policy::PolicyStore::new(db.clone())),
        secrets: workspace_secrets.clone(),
        sessions: sessions.clone(),
        // Identity, for the Ranger auto-admin seed on governed-engine provision:
        // list platform admins (roles) → resolve engine usernames (users).
        users: egret_auth::users::UserRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), bus.clone()),
        pdp: pdp.clone(),
        engine_provisioner,
        kube,
        coordinator_transport,
        // The native monitor signs in to a JWT-delegation engine as the viewer.
        engine_token_minter: Some(engine_minter.clone()),
        engine_egret_url,
        // Engines trust egret's OWN machine-IdP (the STS): the same issuer that
        // signs catalog + engine tokens, independent of whether external OIDC is
        // configured. This replaces the old Keycloak-realm-derived JWKS/issuer
        // (which went empty whenever `oidc.enabled` was false, breaking
        // jwt-delegation) now that egret itself is the single token issuer.
        // An ADDRESS an engine pod dials, so it stays in-cluster even when the
        // issuer is an ingress hostname: keys are identical either way, and
        // hairpinning the data plane's key fetches through the ingress would
        // make every query depend on the ingress being healthy. The discovery
        // document's own `jwks_uri` follows the issuer instead — that one is
        // dialled by an external client. They are deliberately different values.
        engine_jwks_uri: identity.engine_jwks_uri(),
        // Raw issuer, for StarRocks' `CREATE SECURITY INTEGRATION
        // ... required_issuer=<issuer>` (Trino instead reads its own
        // `trinoConfig.auth.jwt.requiredAudience`/JWKS default per-engine, so
        // it has no equivalent need for the raw issuer here). Byte-compared
        // against the `iss` the minter above stamps, so it MUST be the same
        // value — hence identity, not address.
        engine_oidc_issuer: identity.issuer().to_string(),
        // Gates the Ranger auto-bind on Trino engine create. Disabled by default,
        // so new engines stay unbound (allow-all) and delegated impersonation
        // queries work; an admin who explicitly picks an `authz_service_id` still
        // binds regardless.
        ranger_enabled: config.ranger.enabled,
        // The mode the `/ranger` surface runs in; the page for an external
        // engine states it.
        ranger_surface_mode: match config.ranger.auth_mode {
            egret_config::RangerAuthMode::Required => {
                egret_engine_mgmt::state::RangerSurfaceMode::Required
            }
            egret_config::RangerAuthMode::Anonymous => {
                egret_engine_mgmt::state::RangerSurfaceMode::Anonymous
            }
        },
        // The compiled-in pinned JDBC driver table with this deployment's
        // `[starrocks.jdbc_drivers]` overrides applied (design D5).
        sr_jdbc_drivers: egret_catalog_registry::JdbcDrivers::default()
            .with_overrides(&config.starrocks),
        // The real DDL runner (design D8): a live `mysql_async` connection per
        // statement. Tests substitute `FakeStarRocksDdl`.
        starrocks_ddl: std::sync::Arc::new(egret_engine_mgmt::MysqlDdlRunner),
        // Binding an engine to a profile changes which catalogs that profile
        // reaches, and no grant/rule mutation covers it — so the bind brings
        // the profile's compiled policies up to date itself rather than
        // leaving it stale until someone calls the manual trigger.
        perm_compiler: Some(
            profile_compiler.clone() as std::sync::Arc<dyn egret_engine_mgmt::ProfileCompilerPort>
        ),
        catalog_assignment_compiler: Some(profile_compiler.clone()
            as std::sync::Arc<dyn egret_catalog_registry::CatalogAssignmentCompiler>),
    });

    // DG Task 7: bring every existing delegating Trino engine's Ranger profile
    // up to the per-user baseline Trino 483 checks on every session. An apply
    // seeds it too; this pass is what reaches engines deployed before the
    // baseline existed, with no reapply (each plugin picks the rows up on its
    // next policy poll). Best-effort: a failing profile is logged, not fatal.
    //
    // Not gated on `config.ranger.enabled` (final review F6): the apply path
    // seeds any engine bound to a profile whatever that flag says, and an
    // engine bound while it was on keeps its profile after it is turned off.
    // `seed_engine_user_baseline` skips every engine bound to no profile.
    {
        let st = engine_mgmt
            .as_ref()
            .expect("engine-mgmt state is built just above");
        let (seeded, failed) =
            egret_engine_mgmt::seed_user_baselines_for_existing_engines(st).await;
        tracing::info!(seeded, failed, "per-user Ranger baseline pass");
    }

    // Spec 2026-10-20-ranger-surface-auth A10 (Task 6b): a provisioned engine's
    // previous Ranger credential is retired only once the rollout of the
    // current one is OBSERVED complete. This sweep is what observes it (a read
    // of the engine does too). On every replica: the retire is a
    // compare-and-set, so two replicas retire a credential once. Stops with
    // the other background loops.
    {
        let st = engine_mgmt
            .as_ref()
            .expect("engine-mgmt state is built just above")
            .clone();
        egret_engine_mgmt::ranger_credential::spawn_ranger_retire_sweeper(
            st,
            egret_engine_mgmt::ranger_credential::RANGER_RETIRE_SWEEP_INTERVAL,
            reaper_cancel.clone().cancelled_owned(),
        );
    }

    // Control plane: global platform-admin Storage + Secrets + Access console.
    // Keeps `storage` (CatalogStore) + `browser` for the Storage router's
    // credential-rotation push-down onto provisioned warehouses; the warehouse
    // admin router itself moved to egret-catalog-engine (E3d-3, see
    // `catalog_admin` below).
    let control_plane = Some(egret_controlplane::CpState {
        storage: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        users: egret_auth::users::UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), bus.clone()),
        access_resolver: egret_auth::effective::AccessResolver::new(db.clone()),
        secrets: workspace_secrets.clone(),
        sessions: sessions.clone(),
        browser: catalog_browser.clone(),
        pdp: pdp.clone(),
        audit: audit_sink.clone(),
        audit_store: audit_store.clone(),
        tags: egret_bi::TagRepo::new(db.clone()),
        // SAME `bus` as `roles` so a classification-policy change publishes the
        // RolesChanged reload the Cedar PDP subscribes to (Access P3.3).
        classification_policies: egret_bi::ClassificationPolicyRepo::new(db.clone(), bus.clone()),
        db: db.clone(),
        // The classification page's materialized-view report dials the same
        // engines, through the same admin connection, as the catalog apply path
        // — hence built from the engine-mgmt state, and placed after it.
        mv_list_timeout: std::time::Duration::from_secs(15),
        mv_lister: std::sync::Arc::new(EngineMaterializedViewLister {
            engine_mgmt: engine_mgmt
                .clone()
                .expect("engine-mgmt state is built just above"),
        }),
        lineage_sweeper: lineage_sweeper.clone(),
        view_catalog: view_catalog.clone(),
        // The same budget as the StarRocks half's listings.
        view_audit_timeout: std::time::Duration::from_secs(15),
        // Governance Data browse (DG frontend redesign #1): a non-egret
        // catalog is listed on Trino as `egret-lineage` (the lineage EXPLAIN's
        // principal and token) or on StarRocks as `root` (the MV report's
        // connection). The route bounds each call by `browse_timeout`; the
        // Trino statement carries the same budget.
        catalog_lister: std::sync::Arc::new(EngineCatalogLister {
            trino: egret_query::execution::catalog_listing::TrinoCatalogLister::new(
                db.pool.clone(),
                std::sync::Arc::new(egret_bin::engine_minter::MachineIdpEngineMinter::new(
                    egret_machine_idp::PgSigningKeyStore::new(db.clone()),
                    egret_auth::users::UserRepo::new(db.clone()),
                    egret_auth::groups::GroupRepo::new(db.clone()),
                    identity.issuer().to_string(),
                    900,
                )),
                std::time::Duration::from_secs(15),
            ),
            engine_mgmt: engine_mgmt
                .clone()
                .expect("engine-mgmt state is built just above"),
        }),
        // Spec §2.3: 15 s per call, the view audit's budget.
        browse_timeout: std::time::Duration::from_secs(15),
        // DG #3: what `GET /governance/changes` reports as the retention. The
        // same decision as the prune scheduled above, which runs only when
        // audit is enabled; otherwise nothing prunes.
        audit_retention_days: if config.audit.enabled {
            egret_dataapi::serving::scheduled_retention(config.audit.retention_days)
        } else {
            None
        },
        // KI-206: Storage Browse and Download keep the prefixes egret writes
        // in its own bucket closed to anyone but a platform admin.
        storage_scope: egret_controlplane::StorageScope::s3(
            egret_controlplane::SystemPrefixes::new(
                &config.storage.bucket,
                [
                    config.engine.results_prefix.as_str(),
                    config.apps.storage.prefix.as_str(),
                    config.workflow.storage.prefix.as_str(),
                ],
            ),
        ),
    });

    // egret mini machine-IdP: mints Catalog Client tokens (/oauth/token + JWKS).
    // EB-3: the real ClientVerifier is the Catalog Client registry (CatalogStore).
    // `issuer` is egret's public root URL (the JWT `iss` + OIDC issuer lakekeeper
    // will trust in EB-3's Authorizer integration).
    // One key store, shared: the IdP signs with it and the Data API serving path
    // (P4) verifies against it, so there is a single source of truth for what
    // egret's signing key currently is. This is NOT a rotation fix — both sides
    // read `active()` and would follow a rotation forward together either way;
    // see `DataApiState::keys` for what closing that would actually take.
    let signing_keys: std::sync::Arc<dyn egret_machine_idp::SigningKeyStore> =
        std::sync::Arc::new(egret_machine_idp::PgSigningKeyStore::new(db.clone()));
    // Built through `EgretIdentity::idp_state` rather than as a struct literal
    // so that "the IdP's `issuer` is egret's identity" is a line a test
    // executes: it is the `iss` of every minted token AND the `issuer` of the
    // discovery document, and those two disagreeing is precisely the defect
    // this seam exists to prevent.
    let idp = Some(identity.idp_state(
        signing_keys.clone(),
        vec![
            std::sync::Arc::new(egret_catalog_engine::CatalogStore::new(db.clone())),
            // Data API (P2 Task 4): a second, independent audience on the same
            // IdP — a service account's client_id/client_secret exchanges here
            // for an `aud: "data-api"` token instead of `aud: "catalog"`.
            std::sync::Arc::new(egret_dataapi::ClientStore::new(db.clone())),
        ],
        // Its own limiter, not the login one: a shared instance would let
        // machine token traffic evict human login attempts from the same map,
        // and the two limits are deliberately different (see `RateLimitConfig`).
        RateLimiter::new(
            config.rate_limit.token_max_attempts,
            config.rate_limit.token_window_secs,
            config.rate_limit.token_lockout_secs,
        ),
    ));

    // Data API management surface (P2 Task 5). `issuer` is the SAME public root
    // URL the machine-IdP signs with, so a trial token's `iss` matches every
    // other token this deployment signs; `consumer_base_url` is the address a
    // caller outside the cluster uses, so the `token_endpoint` handed back with
    // a new client points somewhere they can actually reach.
    // Data Apps and Workflow Management share the SAME object store
    // (this `S3Store`, the `[storage]` bucket -- one client, not a second one).
    let apps_storage = query_port.storage.clone();
    // `query_port` (the ONE `Arc<EngineState>`) is built above, before
    // `runtime::handlers_for`, which hands it to `egret.sql` (package #4 Task
    // C5); the Data API and the Data Apps gateway below clone the same one.
    let dataapi = Some(egret_dataapi::DataApiState {
        db: db.clone(),
        clients: egret_dataapi::ClientStore::new(db.clone()),
        endpoints: egret_dataapi::EndpointStore::new(db.clone()),
        // The Query seam as a PORT, so `egret-dataapi` never names `egret-query`:
        // publish resolves an endpoint's output columns through `describe_as` and
        // freezes them into the snapshot. This is the one place the concrete
        // `EngineState` and the trait object meet.
        query: query_port.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        pdp: pdp.clone(),
        // The SAME store the machine-IdP above signs with (P4 Task 2), so the
        // serving path verifies a token against the key that actually signed it.
        keys: signing_keys.clone(),
        // A token CLAIM (the trial token's `iss`), which is why it is not the
        // value the URLs below are built from — and why it is egret's identity
        // rather than the in-cluster address.
        issuer: identity.issuer().to_string(),
        // Every URL handed to a consumer or shown to the author. Equal to
        // `issuer` unless `data_api.public_url` is set.
        consumer_base_url: dataapi_consumer_url,
        // The SAME sink `CpState` and every other seam receives (P3 Task 5), so
        // publishing an endpoint or minting a client lands in the one `audit_log`
        // an operator already queries, not in a Data-API-local trail.
        audit: audit_sink.clone(),
        // The serving surface's cache and rate-limit buckets (P4 Task 4).
        // Constructed ONCE, above: built per request they would be a cache that
        // never hits and a limit nobody reaches. Process-local or shared across
        // replicas depending on `choose_store` — see there for the refusal that
        // stops a multi-replica deployment from getting the process-local pair.
        cache: dataapi_cache,
        rate_limits: dataapi_rate_limits,
        // The SAME guard the prune scheduler above was built with (recomputed
        // here rather than shared, since that `retention` binding is local to
        // its own block): the UI is told what is actually happening — whether a
        // prune is scheduled at all — not what was merely configured.
        activity: egret_dataapi::ActivityStore::new(
            db.clone(),
            egret_dataapi::serving::scheduled_retention(config.data_api.access_log_retention_days),
        ),
        // J-F3 Task 1b: a service-account delete leaves every group it was in.
        group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
    });

    // Workflow management surface (W1 Task 7). `refs` is the real
    // `WorkflowRefPort`: `RefKind::DataApiEndpoint` through the port already
    // wired above for `automation`/`dataapi`, `RefKind::Database` through the
    // shared `databases` repo (package #4 Task C5), and `RefKind::Dataset`/
    // `Chart` resolve to `None` always -- no port for either exists anywhere
    // in the workspace yet, and `None` is the fail-closed answer `publish`
    // needs: see `egret_workflow::refs`'s module doc for the full per-kind
    // table.
    //
    // A fresh `EndpointStore`, same reasoning `automation`/`automation_runner`
    // above give for their own fresh instances rather than reusing
    // `dataapi.endpoints`: it is `Clone` and holds only `db`, so a second
    // instance is free, and it keeps this block constructible without caring
    // about `dataapi`'s construction order.
    let workflow = Some(egret_workflow::WorkflowState {
        db: db.clone(),
        repo: egret_workflow::FlowRepo::new(db.clone()),
        executions: egret_workflow::ExecutionRepo::new(db.clone()),
        logs: egret_workflow::logs::LogRepo::new(db.pool.clone()),
        refs: std::sync::Arc::new(egret_workflow::WorkflowRefs {
            dataapi_endpoints: std::sync::Arc::new(egret_dataapi::EndpointStore::new(db.clone())),
            // Package #4 Task C5: an `egret.sql` task's `database:`.
            databases: std::sync::Arc::new(databases.clone()),
        }),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        pdp: pdp.clone(),
        audit: audit_sink.clone(),
        // W5a: the allowlist publish gates script tasks with, and the runner
        // a script task with no `taskRunner:` runs on -- the one
        // `handlers_for` built above, `None` with `container_runner = none`
        // (publish then refuses such a task: nothing would run it).
        scripts: egret_workflow::scripts::ScriptSettings {
            plugin_configurations: config.workflow.plugin_configurations.clone(),
            default_runner: container_runner,
        },
        upload_idle_timeout: egret_workflow::routes::UPLOAD_IDLE_TIMEOUT,
        upload_total_timeout: egret_workflow::routes::UPLOAD_TOTAL_TIMEOUT,
        export_total_timeout: egret_workflow::routes::EXPORT_TOTAL_TIMEOUT,
        // W7e: echoed by the trigger routes, so the UI can say a trigger
        // cannot fire on this deployment.
        trigger_flags: egret_workflow::routes::TriggerFlags {
            scheduler_enabled: config.workflow.scheduler_enabled,
            worker_enabled: config.workflow.worker_enabled,
            webhooks_enabled: config.workflow.webhooks_enabled,
        },
    });

    // Data Apps query gateway (`POST /api/apps/query`). Same query engine `Arc`,
    // audit sink, PDP and Db as every other seam.
    //
    // The limiter's lockout is `rate_window_secs`, NOT 0. `RateLimiter::new`
    // accepts 0, but `check_at` then clears a key's attempts on the very next
    // call after a denial (`now >= locked_until` immediately), so a token would
    // get `rate_max` calls, one 429, then `rate_max` more — the limit would
    // barely bind. A lockout of one window keeps a token at `rate_max` calls
    // per window, which is what `AppsConfig::rate_max` documents.
    let apps = Some(egret_apps::AppsState {
        db: db.clone(),
        pdp: pdp.clone(),
        query: query_port.clone(),
        audit: audit_sink.clone(),
        sessions: egret_apps::session::AppSessionRepo::new(db.clone()),
        limiter: RateLimiter::new(
            config.apps.rate_max,
            config.apps.rate_window_secs,
            config.apps.rate_window_secs,
        ),
        cfg: config.apps.clone(),
    });

    // Dead app tokens and grants (revoked or expired more than a day ago) are deleted hourly. A
    // fresh row is kept a day: it is what explains a 401 someone is still debugging.
    {
        let sessions = egret_apps::session::AppSessionRepo::new(db.clone());
        let grants = egret_apps::grant::GrantRepo::new(db.clone());
        tokio::spawn(async move {
            let older_than = std::time::Duration::from_secs(24 * 3600);
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tick.tick().await;
                match sessions.sweep_expired(older_than).await {
                    Ok(n) if n > 0 => tracing::info!(deleted = n, "app sessions swept"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "app session sweep failed"),
                }
                match grants.sweep(older_than).await {
                    Ok(n) if n > 0 => tracing::info!(deleted = n, "app grants swept"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "app grant sweep failed"),
                }
            }
        });
    }

    // Data Apps management API (`/api/ws/{workspace}/apps…`): the same session
    // service, workspace repo, authorizer, PDP, audit sink and Db every other
    // feature seam shares.
    let apps_api = Some(egret_apps::AppsApiState {
        store: egret_apps::store::AppStore::new(
            db.clone(),
            apps_storage.clone(),
            config.storage.bucket.clone(),
            config.apps.storage.prefix.clone(),
        ),
        db: db.clone(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        pdp: pdp.clone(),
        audit: audit_sink.clone(),
        grants: egret_apps::grant::GrantRepo::new(db.clone()),
        apps_origin: config.apps.origin.clone(),
        ticket_ttl: std::time::Duration::from_secs(config.apps.ticket_ttl_secs),
    });

    // Data Apps pod API (`/api/apps/pod/…`): what an app pod calls with its pod token.
    let apps_pod = Some(egret_apps::PodApiState {
        db: db.clone(),
        pdp: pdp.clone(),
        pods: egret_apps::podtoken::PodTokenRepo::new(db.clone()),
        store: egret_apps::store::AppStore::new(
            db.clone(),
            apps_storage.clone(),
            config.storage.bucket.clone(),
            config.apps.storage.prefix.clone(),
        ),
    });

    // The apps host (spec §6.2, D17): the ticket route and the proxy, on a listener of their
    // own. Off unless `apps.origin` is set. Which pods it can reach depends on
    // `apps.pod.launcher`: with `none`, every pod is unavailable (the cookie, route and header
    // checks are live all the same).
    let apps_proxy = if config.apps.origin.is_empty() {
        None
    } else {
        let pod = &config.apps.pod;
        let pod_tokens = egret_apps::podtoken::PodTokenRepo::new(db.clone());
        let launcher: Option<std::sync::Arc<dyn egret_apps::AppPodLauncher>> =
            match pod.launcher.as_str() {
                "subprocess" => Some(std::sync::Arc::new(
                    egret_apps::subprocess_pod::SubprocessPodLauncher::new(
                        pod_tokens,
                        egret_apps::subprocess_pod::SubprocessConfig {
                            command: pod.command.clone(),
                            work_dir: pod.work_dir.clone().into(),
                            config_dir: pod.config_dir.clone().into(),
                            venv_shim_dir: pod.venv_shim_dir.clone().into(),
                            callback_url: pod.callback_url.clone(),
                            run_kernel_mode: pod.run_kernel_mode.clone(),
                        },
                    ),
                )),
                "kubernetes" => {
                    let client = kube::Client::try_default()
                        .await
                        .expect("apps.pod.launcher=kubernetes but no in-cluster kube config");
                    Some(std::sync::Arc::new(
                        crate::apps_pod_k8s::KubernetesAppPodLauncher::new(
                            client,
                            crate::apps_pod_k8s::K8sAppPodConfig {
                                namespace: pod.namespace.clone(),
                                image: pod.image.clone(),
                                image_pull_policy: pod.image_pull_policy.clone(),
                                cpu_request: pod.cpu_request.clone(),
                                mem_request: pod.mem_request.clone(),
                                cpu_limit: pod.cpu_limit.clone(),
                                mem_limit: pod.mem_limit.clone(),
                                pull_secret: pod.pull_secret.clone(),
                                callback_url: pod.callback_url.clone(),
                                run_kernel_mode: pod.run_kernel_mode.clone(),
                            },
                            pod_tokens,
                        ),
                    ))
                }
                _ => None,
            };
        let resolver: std::sync::Arc<dyn egret_apps::PodResolver> = match launcher {
            Some(launcher) => {
                tracing::info!(mode = launcher.mode(), "app pod launcher");
                egret_apps::spawn_pod_reaper(
                    launcher.clone(),
                    std::time::Duration::from_secs(pod.idle_secs),
                    std::time::Duration::from_secs(pod.run_idle_secs),
                    std::time::Duration::from_secs(60),
                );
                std::sync::Arc::new(egret_apps::LauncherPods {
                    launcher,
                    ready_timeout: std::time::Duration::from_secs(pod.ready_timeout_secs),
                })
            }
            None => std::sync::Arc::new(egret_apps::UnavailablePods),
        };
        Some(egret_apps::proxy_router(egret_apps::ProxyState::new(
            db.clone(),
            pdp.clone(),
            egret_apps::grant::GrantRepo::new(db.clone()),
            egret_apps::session::AppSessionRepo::new(db.clone()),
            resolver,
            egret_apps::ProxyConfig {
                egret_origin: config.apps.egret_origin.clone(),
                secure: config.apps.origin.starts_with("https://"),
                session_idle: std::time::Duration::from_secs(config.apps.session_idle_secs),
                session_absolute: std::time::Duration::from_secs(config.apps.session_absolute_secs),
                cookie_ttl: std::time::Duration::from_secs(config.apps.cookie_ttl_secs),
                run_session_cap: pod.run_session_cap,
            },
        )))
    };

    let app = build_router(AppState {
        db: Some(db),
        auth: Some(auth),
        workspace: Some(workspace),
        oidc,
        ranger,
        database,
        engine,
        bi,
        automation,
        control_plane,
        catalog_admin,
        policy_admin,
        perm,
        engine_mgmt,
        catalog_engine,
        idp,
        dataapi,
        workflow,
        // **The anonymous webhook surface is OFF unless an operator says
        // otherwise.** See `egret_server::AppState::workflow_webhooks_enabled`:
        // the key in the URL is the whole credential, the surface's own
        // security review says it is not approved for exposure, and before fix
        // round 2 that verdict had no lever at all.
        //
        // It comes through `Config` like every other deployment switch —
        // `[workflow] webhooks_enabled`, i.e.
        // `EGRET_WORKFLOW__WEBHOOKS_ENABLED`, the `EGRET_<SECTION>__<KEY>`
        // convention `EGRET_CATALOG__ENABLED` and `EGRET_OIDC__ENABLED`
        // already follow. Fix round 2 shipped it as a flat `std::env::var`
        // read here, which was outside the config type, outside the chart and
        // outside the convention (fix round 3, Minors 2 and 3). figment does
        // the string→bool step, so a value it cannot parse is a **startup
        // error** rather than a silent `false`: an operator who meant to turn
        // this on and mistyped finds out at boot.
        workflow_webhooks_enabled: config.workflow.webhooks_enabled,
        apps,
        apps_api,
        apps_pod,
    });
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "egret listening");
    if let Some(router) = apps_proxy {
        let apps_addr = config.apps.listen.clone();
        let apps_listener = tokio::net::TcpListener::bind(&apps_addr)
            .await
            .with_context(|| format!("binding apps host {apps_addr}"))?;
        tracing::info!(addr = %apps_addr, origin = %config.apps.origin, "apps host listening");
        tokio::spawn(async move {
            if let Err(e) = egret_server::serve_with_drain(
                apps_listener,
                router,
                egret_server::shutdown_signal(),
                egret_server::DRAIN_DEADLINE,
            )
            .await
            {
                tracing::error!(error = %e, "apps host stopped");
            }
        });
    }
    // SIGTERM is observed and in-flight requests are drained, bounded by
    // `DRAIN_DEADLINE`. Before this, a rollout dropped every in-flight query on
    // the floor — and silently, because `dataapi.access_log` is written after
    // the response, so the killed requests appeared in no activity view.
    egret_server::serve_with_drain(
        listener,
        app,
        egret_server::shutdown_signal(),
        egret_server::DRAIN_DEADLINE,
    )
    .await
    .context("server error")?;
    // The workflow claim loop, then its log writer's drain (bounded at 10 s by
    // the writer itself). Bounded here too: a tick in flight can be a long
    // `Sleep`, and a pod's grace period is finite.
    reaper_cancel.cancel();
    if tokio::time::timeout(WORKFLOW_STOP_WAIT, workflow_runtime.join())
        .await
        .is_err()
    {
        tracing::warn!(
            wait_secs = WORKFLOW_STOP_WAIT.as_secs(),
            "workflow runtime did not stop in time; queued log lines may be lost"
        );
    }
    tracing::info!("egret stopped");
    Ok(())
}

/// How long `main` waits, after the HTTP drain, for the workflow claim loop
/// and its log writer to stop. The writer's own drain is bounded at 10 s; the
/// server's drain (55 s) plus this stays inside the chart's 65 s grace period.
const WORKFLOW_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(test)]
mod catalog_authz_tests {
    use super::{
        catalog_grant_covers, is_recursive, is_signer_route, path_namespace, path_tabular,
    };
    use egret_catalog_engine::CatalogGrant;
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
    fn a_recursive_drop_is_read_as_lakekeeper_reads_the_query() {
        assert!(is_recursive(Some("recursive=true")));
        assert!(is_recursive(Some("purge=true&recursiv%65=true")));
        assert!(
            is_recursive(Some("recursive=TRUE")),
            "refused by lakekeeper; counted"
        );
        assert!(!is_recursive(Some("recursive=false")));
        assert!(
            !is_recursive(Some("Recursive=true")),
            "an unknown key to lakekeeper"
        );
        assert!(!is_recursive(None));
    }

    #[test]
    fn every_signer_route_is_recognised() {
        for p in [
            "/aws/s3/sign",
            "/wh/v1/aws/s3/sign",
            "/signer/wh/tabular-id/x/v1/aws/s3/sign",
            "/signer/anything",
            "//wh//v1/aws/s3/sign/",
        ] {
            assert!(is_signer_route(p), "{p}");
        }
        for p in [
            "/config",
            "/wh/namespaces/aws/tables/sign",
            "/wh/tables/rename",
        ] {
            assert!(!is_signer_route(p), "{p}");
        }
    }

    #[test]
    fn ns_parts_handles_multipart() {
        assert_eq!(path_namespace("demo_ns"), Some(vec!["demo_ns".to_string()]));
        assert_eq!(
            path_namespace("a%1Fb"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            path_namespace("a%1fb+c"),
            Some(vec!["a".to_string(), "b c".to_string()])
        );
        assert_eq!(path_tabular("caf%C3%A9+x%2B"), Some("café x ".to_string()));
        assert_eq!(path_tabular("bad%zz"), None);
        assert_eq!(path_tabular("%FF"), None);
    }
}

/// The catalog guard must reject a token minted for a DIFFERENT audience on the
/// same signing key.
///
/// DB-gated (the guard needs `PgSigningKeyStore`, whose key lives in Postgres);
/// self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS, like the other DB-backed suites.
#[cfg(test)]
mod catalog_guard_body_tests;

/// Vended S3 credentials follow the caller's grants (F4), through the real
/// guard and lakekeeper's router on egret's S3 profile over an in-process
/// object store and STS.
#[cfg(test)]
mod catalog_vended_credentials_tests;

#[cfg(test)]
mod catalog_guard_audience_tests {
    use super::{EgretCatalogGuard, CATALOG_AUDIENCE};
    use egret_catalog_engine::CatalogGuard as _;
    use egret_machine_idp::SigningKeyStore as _;

    async fn guard() -> Option<EgretCatalogGuard> {
        let url = egret_test_db::url().await?;
        let db = egret_metastore::Db::connect(&url, 5)
            .await
            .expect("connect");
        db.migrate().await.expect("migrate");
        Some(EgretCatalogGuard {
            keys: egret_machine_idp::PgSigningKeyStore::new(db.clone()),
            catalog: egret_catalog_engine::CatalogStore::new(db),
        })
    }

    /// Sign a well-formed, unexpired token with the LIVE signing key, carrying
    /// `aud`. Everything except the audience is valid, so a rejection can only
    /// be the audience check.
    async fn token_with_aud(g: &EgretCatalogGuard, aud: &str) -> String {
        let km = g.keys.active().await.expect("active key");
        let exp = chrono::Utc::now().timestamp() + 600;
        egret_machine_idp::sign_claims(
            &km,
            &serde_json::json!({ "sub": "c", "aud": aud, "exp": exp }),
        )
        .expect("sign")
    }

    /// `GET /config` with no `warehouse=` is the arm that returns `Ok(())`
    /// without ever consulting the `warehouse` claim — the one place a
    /// foreign-audience token reaches lakekeeper if the audience is unchecked.
    /// Both legs are asserted: without the "before" leg this would pass just as
    /// well against a guard that rejected everything.
    #[tokio::test]
    async fn config_rejects_a_data_api_token_but_accepts_a_catalog_one() {
        let Some(g) = guard().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };

        let catalog_tok = token_with_aud(&g, CATALOG_AUDIENCE).await;
        assert!(
            g.check("GET", "/config", None, Some(&catalog_tok), None)
                .await
                .is_ok(),
            "a catalog-audience token must still pass /config"
        );

        let data_api_tok = token_with_aud(&g, "data-api").await;
        let err = g
            .check("GET", "/config", None, Some(&data_api_tok), None)
            .await
            .expect_err("a data-api token must not reach the catalog");
        assert_eq!(err.0, axum::http::StatusCode::UNAUTHORIZED, "{}", err.1);
    }

    /// Rename spec R7: the guard returns who the verified token names, so the
    /// table hook can record the caller of a classification move, and the
    /// mount can name its warehouse and role to lakekeeper's authorizer.
    #[tokio::test]
    async fn the_catalog_guard_returns_the_token_subject() {
        let Some(g) = guard().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let km = g.keys.active().await.expect("active key");
        let token = egret_machine_idp::sign_claims(
            &km,
            &serde_json::json!({
                "sub": "cat1", "aud": CATALOG_AUDIENCE,
                "warehouse": "wh1", "roles": ["r1"],
                "exp": chrono::Utc::now().timestamp() + 600,
            }),
        )
        .expect("sign");
        let caller = g
            .check("GET", "/config", None, Some(&token), None)
            .await
            .expect("a catalog token passes /config");
        assert_eq!(
            caller,
            egret_catalog_engine::CatalogCaller {
                subject: "cat1".into(),
                audience: CATALOG_AUDIENCE.into(),
                warehouse: "wh1".into(),
                role: "r1".into(),
            }
        );
    }

    /// Final review M-7: the view hold passes an unknown prefix through
    /// unheld, which is safe only because this guard refuses any prefix that
    /// is not byte-identical to the warehouse's `lakekeeper_warehouse_id`.
    /// lakekeeper itself accepts the uppercase, simple and `urn:` forms of the
    /// same UUID, so each of those must be a 403 here, before the hold runs.
    /// The canonical prefix is the control: it passes the prefix check and is
    /// refused only for having no grant.
    #[tokio::test]
    async fn an_alias_of_the_warehouse_prefix_is_refused_before_the_hold() {
        let Some(g) = guard().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let pool = g.catalog.pool();
        let s = uuid::Uuid::new_v4().simple().to_string();
        let ws: uuid::Uuid =
            sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'G') RETURNING id")
                .bind(format!("guard-ws-{s}"))
                .fetch_one(pool)
                .await
                .expect("workspace");
        let sc: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
             VALUES ($1,'us-east-1','b','AK') RETURNING id",
        )
        .bind(format!("guard-sc-{s}"))
        .fetch_one(pool)
        .await
        .expect("storage connection");
        let lakekeeper_id = uuid::Uuid::new_v4();
        let warehouse = format!("guard_wh_{s}");
        sqlx::query(
            "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id, \
             lakekeeper_warehouse_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(&warehouse)
        .bind(ws)
        .bind(sc)
        .bind(lakekeeper_id.to_string())
        .execute(pool)
        .await
        .expect("warehouse");
        let km = g.keys.active().await.expect("active key");
        let token = egret_machine_idp::sign_claims(
            &km,
            &serde_json::json!({
                "sub": "c", "aud": CATALOG_AUDIENCE, "warehouse": warehouse,
                "roles": ["guard-role"], "exp": chrono::Utc::now().timestamp() + 600,
            }),
        )
        .expect("sign");

        let canonical = lakekeeper_id.hyphenated().to_string();
        let err = g
            .check(
                "POST",
                &format!("/{canonical}/namespaces/s/views"),
                None,
                Some(&token),
                None,
            )
            .await
            .expect_err("no grant");
        assert!(
            err.1.contains("has no grants"),
            "the canonical prefix passed: {}",
            err.1
        );

        for alias in [
            canonical.to_uppercase(),
            lakekeeper_id.simple().to_string(),
            format!("urn:uuid:{canonical}"),
        ] {
            let err = g
                .check(
                    "POST",
                    &format!("/{alias}/namespaces/s/views"),
                    None,
                    Some(&token),
                    None,
                )
                .await
                .expect_err("an alias prefix must be refused");
            assert_eq!(
                err.0,
                axum::http::StatusCode::FORBIDDEN,
                "{alias}: {}",
                err.1
            );
            assert_eq!(err.1, "token is not scoped to this warehouse", "{alias}");
        }
    }
}

/// The Governance browse route's `CatalogLister` (DG frontend redesign #1):
/// Trino through `egret-query`, StarRocks through `egret-engine-mgmt`, neither
/// of which the control plane depends on. Both answer an
/// `egret_query_api::catalog_listing::CatalogLevel`, mapped here.
struct EngineCatalogLister {
    trino: egret_query::execution::catalog_listing::TrinoCatalogLister,
    engine_mgmt: egret_engine_mgmt::EngineMgmtState,
}

#[async_trait::async_trait]
impl egret_controlplane::CatalogLister for EngineCatalogLister {
    async fn list(
        &self,
        engine: &egret_controlplane::ListingEngine,
        catalog: &str,
        schema: Option<&str>,
        table: Option<&str>,
    ) -> Result<egret_controlplane::Listing, String> {
        let level = match engine.engine_type {
            egret_controlplane::ListingEngineType::Trino => {
                self.trino
                    .list(&engine.name, catalog, schema, table)
                    .await?
            }
            egret_controlplane::ListingEngineType::StarRocks => {
                egret_engine_mgmt::list_catalog_level(
                    &self.engine_mgmt,
                    engine.id,
                    catalog,
                    schema,
                    table,
                )
                .await?
            }
        };
        Ok(listing_of(level))
    }
}

/// An engine's [`CatalogLevel`](egret_query_api::catalog_listing::CatalogLevel)
/// as the browse route's `Listing`.
fn listing_of(
    level: egret_query_api::catalog_listing::CatalogLevel,
) -> egret_controlplane::Listing {
    use egret_controlplane::{BrowseColumn, BrowseItem, BrowseKind, Listing};
    use egret_query_api::catalog_listing::{CatalogLevel, ListedKind};
    match level {
        CatalogLevel::Schemas(names) => Listing::Items(
            names
                .into_iter()
                .map(|name| BrowseItem {
                    name,
                    kind: BrowseKind::Schema,
                })
                .collect(),
        ),
        CatalogLevel::Relations(relations) => Listing::Items(
            relations
                .into_iter()
                .map(|r| BrowseItem {
                    name: r.name,
                    kind: match r.kind {
                        ListedKind::Table => BrowseKind::Table,
                        ListedKind::View => BrowseKind::View,
                        ListedKind::MaterializedView => BrowseKind::MaterializedView,
                    },
                })
                .collect(),
        ),
        CatalogLevel::Columns(columns) => Listing::Columns(
            columns
                .into_iter()
                .map(|c| BrowseColumn {
                    name: c.name,
                    r#type: c.r#type,
                })
                .collect(),
        ),
    }
}

/// Lists a StarRocks database's materialized views through the engine-mgmt
/// admin connection (view governance A4). The control plane owns the route and
/// cannot depend on the engine-mgmt crate, so the composition root bridges them.
struct EngineMaterializedViewLister {
    engine_mgmt: egret_engine_mgmt::EngineMgmtState,
}

#[async_trait::async_trait]
impl egret_controlplane::MaterializedViewLister for EngineMaterializedViewLister {
    async fn list(
        &self,
        engine: &egret_controlplane::ExposureEngine,
    ) -> Result<Vec<egret_controlplane::MvRow>, String> {
        Ok(
            egret_engine_mgmt::list_materialized_views(&self.engine_mgmt, engine.id)
                .await?
                .into_iter()
                .map(|r| egret_controlplane::MvRow {
                    database: r.database,
                    name: r.name,
                    base_tables: r.base_tables,
                    definition: r.definition,
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod provisioner_selection_tests {
    use super::{select_provisioner, Selection};

    fn engines_cfg(p: egret_config::Provisioner) -> egret_config::EnginesConfig {
        egret_config::EnginesConfig {
            provisioner: p,
            ..Default::default()
        }
    }

    #[test]
    fn provisioner_selection_follows_the_config() {
        use egret_config::Provisioner::*;
        let sel = |p, kube| select_provisioner(&engines_cfg(p), kube);
        assert_eq!(sel(Auto, true), Ok(Selection::Kubernetes));
        assert_eq!(sel(Auto, false), Ok(Selection::Disabled));
        assert_eq!(sel(Kubernetes, true), Ok(Selection::Kubernetes));
        assert!(sel(Kubernetes, false).is_err());
        // Docker is never auto-selected, but explicit wins whatever the cluster.
        assert_eq!(sel(Docker, true), Ok(Selection::Docker));
        assert_eq!(sel(Docker, false), Ok(Selection::Docker));
        assert_eq!(sel(None, true), Ok(Selection::Disabled));
        assert_eq!(sel(None, false), Ok(Selection::Disabled));
    }
}
