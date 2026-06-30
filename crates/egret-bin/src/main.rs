//! `egret` binary entrypoint: parse the CLI, load config, dispatch the command.

mod cli;

use anyhow::Context;
use chrono::Duration;
use clap::Parser;
use cli::{parse_namespaces, CatalogAction, Cli, Command};
use egret_auth::groups::GroupRepo;
use egret_auth::password::hash_password;
use egret_auth::ratelimit::RateLimiter;
use egret_auth::routes::AuthState;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
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
    let cli = Cli::parse();
    let config = Config::load().context("loading configuration")?;
    config.validate().map_err(|e| anyhow::anyhow!(e))?;
    init_tracing(&config);

    match cli.command {
        Command::Serve => serve(config).await,
        Command::Migrate => migrate(config).await,
        Command::Init { force } => init(config, force).await,
        Command::Catalog { action } => catalog_cmd(config, action).await,
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

async fn catalog_cmd(config: Config, action: CatalogAction) -> anyhow::Result<()> {
    let db = connect_db(&config).await?;
    let store = CatalogStore::new(db.clone());
    let workspaces = WorkspaceRepo::new(db.clone());

    match action {
        CatalogAction::MintToken {
            workspace,
            warehouse,
            label,
            perms,
            namespaces,
        } => {
            // Parse --perms: comma-separated "read" and/or "write" tokens.
            let permissions: Vec<String> = perms
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(|s| match s.as_str() {
                    "read" => Ok("catalog:read".to_string()),
                    "write" => Ok("catalog:write".to_string()),
                    other => Err(anyhow::anyhow!(
                        "invalid permission '{other}': must be 'read' or 'write'"
                    )),
                })
                .collect::<anyhow::Result<Vec<String>>>()?;

            if permissions.is_empty() {
                anyhow::bail!("--perms must not be empty; use 'read', 'write', or 'read,write'");
            }

            // Parse --namespaces: comma-separated namespace names; empty = all.
            let namespace_scope = parse_namespaces(&namespaces);

            // Resolve workspace slug → workspace_id.
            let ws = workspaces
                .get_by_slug(&workspace)
                .await
                .context("looking up workspace")?
                .ok_or_else(|| anyhow::anyhow!("workspace not found: {workspace}"))?;

            // Resolve the named warehouse within the workspace (a token is now
            // minted FOR a specific warehouse).
            let wh = store
                .get_warehouse_by_name(ws.id.0, &warehouse)
                .await
                .map_err(|_| {
                    anyhow::anyhow!("warehouse '{warehouse}' not found in workspace '{workspace}'")
                })?;

            // Token scope v2: the CLI mints a single resource grant. Each
            // namespace from `--namespaces` (or the `"*"` all-namespaces sentinel
            // when none are given) becomes a whole-namespace grant carrying the
            // requested permissions, preserving the prior CLI behaviour.
            let scope: Vec<egret_catalog::service::tokens::Grant> = if namespace_scope.is_empty() {
                vec![egret_catalog::service::tokens::Grant {
                    namespace: "*".to_string(),
                    table: None,
                    permissions: permissions.clone(),
                }]
            } else {
                namespace_scope
                    .iter()
                    .map(|ns| egret_catalog::service::tokens::Grant {
                        namespace: ns.clone(),
                        table: None,
                        permissions: permissions.clone(),
                    })
                    .collect()
            };

            let (token_row, raw) = store
                .create_token(ws.id.0, wh.id, &label, &scope)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            println!("Token id      : {}", token_row.id);
            println!("Workspace     : {} ({})", workspace, ws.id.0);
            println!("Warehouse     : {} ({})", warehouse, wh.id);
            println!("Label         : {}", token_row.label);
            println!("Scope         : {}", fmt_scope(&token_row.scope));
            println!("Created at    : {}", token_row.created_at.to_rfc3339());
            println!();
            println!("Raw token (store this now — it will NOT be shown again):");
            println!("{raw}");
        }

        CatalogAction::RevokeToken { id } => {
            store
                .revoke_token(id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("Token {id} revoked.");
        }

        CatalogAction::ListTokens {
            workspace,
            warehouse,
        } => {
            let ws = workspaces
                .get_by_slug(&workspace)
                .await
                .context("looking up workspace")?
                .ok_or_else(|| anyhow::anyhow!("workspace not found: {workspace}"))?;

            let wh = store
                .get_warehouse_by_name(ws.id.0, &warehouse)
                .await
                .map_err(|_| {
                    anyhow::anyhow!("warehouse '{warehouse}' not found in workspace '{workspace}'")
                })?;

            let tokens = store
                .list_tokens_for_warehouse(ws.id.0, wh.id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            if tokens.is_empty() {
                println!("No active tokens for workspace '{workspace}'.");
            } else {
                println!(
                    "{:<38}  {:<20}  {:<40}  CREATED AT",
                    "TOKEN ID", "LABEL", "SCOPE (grants)"
                );
                println!("{}", "-".repeat(120));
                for t in tokens {
                    println!(
                        "{:<38}  {:<20}  {:<40}  {}",
                        t.id,
                        t.label,
                        fmt_scope(&t.scope),
                        t.created_at.to_rfc3339()
                    );
                }
            }
        }
    }

    Ok(())
}

/// Format a token's resource-grant scope for CLI display, e.g.
/// `* (catalog:read)`, `sale (catalog:read)`, `marketing.campaigns (catalog:read,catalog:write)`.
fn fmt_scope(scope: &[egret_catalog::service::tokens::Grant]) -> String {
    scope
        .iter()
        .map(|g| {
            let resource = match &g.table {
                Some(t) => format!("{}.{t}", g.namespace),
                None => g.namespace.clone(),
            };
            format!("{resource} ({})", g.permissions.join(","))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations on boot")?;

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

    // --- Embedded lakekeeper catalog engine (CE-2 T5) ------------------------
    // When the catalog surface is enabled, build the LIVE engine at startup:
    //   1. migrate lakekeeper's catalog into its isolated `lakekeeper` schema,
    //   2. bootstrap the default project under the stable system principal,
    //   3. build the engine `ApiContext` (`build_state`),
    //   4. assemble the engine router fronted by egret's per-workspace
    //      token-ownership middleware (`mount_catalog_engine`, A-3).
    // Workspace creation no longer auto-provisions a warehouse; warehouses are
    // now created on demand via the per-warehouse management API.
    // This is the single composition point that makes the engine live; the
    // lakekeeper graph (and its second `sqlx` major) stays contained here.
    // Hoisted so BOTH the engine block (per-workspace storage secrets, CE-5) and
    // the database block (database-connection secrets) share one secret store
    // built from the same `db` + master key. `SecretStore` is cheap + `Clone`.
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

    // The embedded lakekeeper engine `ApiContext` is also hoisted out of this
    // block so the notebook-runtime namespace provisioner (below) can drive
    // `egret_catalog_engine::ensure_namespace` in-process, with the same system
    // principal the browse API uses.
    type CatalogEngineCtx = egret_catalog_engine::ApiContext<egret_catalog_engine::EngineState>;
    let (catalog_engine, catalog_browse, catalog_engine_ctx): (
        Option<axum::Router>,
        Option<axum::Router>,
        Option<CatalogEngineCtx>,
    ) = if config.catalog.enabled {
        let dsn = &config.database.url;
        egret_catalog_engine::run_lakekeeper_migrations(dsn)
            .await
            .context("running lakekeeper catalog migrations on boot")?;
        let (engine_state, gc_registry) =
            egret_catalog_engine::build_state(dsn, &config.catalog.system_user)
                .await
                .context("building lakekeeper engine state")?;
        egret_catalog_engine::ensure_bootstrapped(
            engine_state.clone(),
            &config.catalog.system_user,
        )
        .await
        .context("bootstrapping lakekeeper default project")?;

        let engine_store = egret_catalog::service::CatalogStore::new(db.clone());

        let router =
            egret_catalog_engine::mount_catalog_engine(engine_state.clone(), engine_store.clone());

        // CE-2 T6: the session-authed browse API (for the UI) reads the embedded
        // lakekeeper catalog via the engine `ApiContext`. Built here (in the
        // composition root) so the lakekeeper graph stays contained, then carried
        // into `AppState` as a pre-built router (like `catalog_engine`).
        let browse_router =
            egret_catalog_engine::browse_router().with_state(egret_catalog_engine::BrowseState {
                engine: engine_state.clone(),
                store: egret_catalog::service::CatalogStore::new(db.clone()),
                workspace_secrets: workspace_secrets.clone(),
                system_user: config.catalog.system_user.clone().into(),
                sessions: sessions.clone(),
                workspaces: WorkspaceRepo::new(db.clone()),
                authz: Authorizer::new(db.clone()),
            });

        let gc_cancel = egret_catalog_engine::CancellationToken::new();
        egret_catalog_engine::spawn_gc_workers(gc_registry, gc_cancel.clone());
        tracing::info!("catalog GC workers started");
        {
            let gc_cancel = gc_cancel.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("SIGINT received, stopping catalog GC workers");
                gc_cancel.cancel();
            });
        }

        (Some(router), Some(browse_router), Some(engine_state))
    } else {
        (None, None, None)
    };

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

    // CE-2 T6: the browse API (`catalog_browse`) is built above alongside the
    // embedded lakekeeper engine — it reads lakekeeper, so it requires the engine
    // `ApiContext` and is therefore gated on `config.catalog.enabled` (when the
    // catalog surface is off there is no warehouse to browse).

    // `secrets` (the SecretStore) was hoisted above the engine block so the
    // per-workspace storage provisioner shares it; the database block reuses the
    // same instance for database-connection secrets.
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

    // T7: the real lakekeeper-backed namespace provisioner for the notebook
    // runtime. It resolves the egret warehouse-row id to its lakekeeper UUID
    // (via `CatalogStore::lakekeeper_id_by_row`) and drives the catalog engine's
    // in-process `ensure_namespace` (= `NamespaceService::create_namespace`,
    // already-exists → Ok) under the configured system principal. Wired only
    // when the catalog engine is enabled; when it is not, namespace creation has
    // no backing engine, so we fall back to a no-op (the notebook runtime cannot
    // mint a usable catalog token without a warehouse anyway).
    struct RealNamespaceProvisioner {
        engine: CatalogEngineCtx,
        catalog: CatalogStore,
        system_user: String,
    }

    #[async_trait::async_trait]
    impl egret_notebook::NamespaceProvisioner for RealNamespaceProvisioner {
        async fn ensure_namespace(
            &self,
            warehouse_id: uuid::Uuid,
            namespace: &str,
        ) -> Result<(), egret_notebook::error::NotebookApiError> {
            use egret_notebook::error::NotebookApiError;
            // egret warehouse-row id → lakekeeper warehouse UUID.
            let lakekeeper_id = self
                .catalog
                .lakekeeper_id_by_row(warehouse_id)
                .await
                .map_err(|e| match e {
                    egret_catalog::CatalogError::NotFound(m) => {
                        NotebookApiError(egret_core::Error::NotFound(m))
                    }
                    other => NotebookApiError(egret_core::Error::Internal(anyhow::anyhow!(other))),
                })?;
            // Idempotent in-process create (already-exists = Ok).
            egret_catalog_engine::ensure_namespace(
                &self.engine,
                &self.system_user,
                lakekeeper_id,
                namespace,
            )
            .await
            .map_err(|e| NotebookApiError(egret_core::Error::Internal(e)))
        }
    }

    /// No-op provisioner used when the catalog engine is disabled (no embedded
    /// lakekeeper to create namespaces in). The notebook runtime endpoint also
    /// has no warehouse to mint a token against in that mode, so this is never
    /// reached on a real provisioning path; it only keeps `NotebookState`
    /// constructible.
    struct NoopNamespaceProvisioner;

    #[async_trait::async_trait]
    impl egret_notebook::NamespaceProvisioner for NoopNamespaceProvisioner {
        async fn ensure_namespace(
            &self,
            _warehouse_id: uuid::Uuid,
            _namespace: &str,
        ) -> Result<(), egret_notebook::error::NotebookApiError> {
            Ok(())
        }
    }

    let provisioner: std::sync::Arc<dyn egret_notebook::NamespaceProvisioner> =
        match catalog_engine_ctx {
            Some(engine) => std::sync::Arc::new(RealNamespaceProvisioner {
                engine,
                catalog: CatalogStore::new(db.clone()),
                system_user: config.catalog.system_user.clone(),
            }),
            None => std::sync::Arc::new(NoopNamespaceProvisioner),
        };

    let catalog_base_url = config
        .catalog
        .public_url
        .clone()
        .unwrap_or_else(|| "http://localhost:8080".into());

    let notebook = Some(egret_notebook::NotebookState {
        store: egret_notebook::NotebookStore::new(db.clone()),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        catalog: CatalogStore::new(db.clone()),
        provisioner,
        users: UserRepo::new(db.clone()),
        catalog_base_url,
    });

    let app = build_router(AppState {
        db: Some(db),
        auth: Some(auth),
        workspace: Some(workspace),
        oidc,
        catalog_engine,
        catalog_browse,
        ranger,
        database,
        engine,
        bi,
        notebook,
    });
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "egret listening");
    axum::serve(listener, app).await.context("server error")?;
    Ok(())
}
