//! `egret` binary entrypoint: parse the CLI, load config, dispatch the command.

mod cli;

use anyhow::Context;
use chrono::Duration;
use clap::Parser;
use cli::{CatalogAction, Cli, Command};
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
        CatalogAction::MintToken { workspace, label } => {
            // Resolve workspace slug → workspace_id.
            let ws = workspaces
                .get_by_slug(&workspace)
                .await
                .context("looking up workspace")?
                .ok_or_else(|| anyhow::anyhow!("workspace not found: {workspace}"))?;

            let (token_row, raw) = store
                .create_token(ws.id.0, &label)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            println!("Token id   : {}", token_row.id);
            println!("Workspace  : {} ({})", workspace, ws.id.0);
            println!("Label      : {}", token_row.label);
            println!("Created at : {}", token_row.created_at.to_rfc3339());
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

        CatalogAction::ListTokens { workspace } => {
            let ws = workspaces
                .get_by_slug(&workspace)
                .await
                .context("looking up workspace")?
                .ok_or_else(|| anyhow::anyhow!("workspace not found: {workspace}"))?;

            let tokens = store
                .list_tokens(ws.id.0)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            if tokens.is_empty() {
                println!("No active tokens for workspace '{workspace}'.");
            } else {
                println!("{:<38}  {:<20}  CREATED AT", "TOKEN ID", "LABEL");
                println!("{}", "-".repeat(80));
                for t in tokens {
                    println!(
                        "{:<38}  {:<20}  {}",
                        t.id,
                        t.label,
                        t.created_at.to_rfc3339()
                    );
                }
            }
        }
    }

    Ok(())
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
    let workspace = WorkspaceState {
        // Provision per-workspace Iceberg catalogs under config.catalog.warehouse.
        workspaces: WorkspaceRepo::with_warehouse_base(
            db.clone(),
            config.catalog.warehouse.clone(),
        ),
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

    let catalog = if config.catalog.enabled {
        let storage_cfg = egret_storage::StorageConfig {
            endpoint: config.storage.endpoint.clone(),
            region: config.storage.region.clone(),
            bucket: config.storage.bucket.clone(),
            access_key: config.storage.access_key.clone(),
            secret_key: config.storage.secret_key.clone(),
            path_style: config.storage.path_style,
        };
        let store = egret_catalog::service::CatalogStore::new(db.clone());
        let storage: std::sync::Arc<dyn egret_storage::ObjectStore> =
            std::sync::Arc::new(egret_storage::S3Store::new(&storage_cfg).await?);
        let service = egret_catalog::service::CatalogService { store, storage };
        // Advertise an externally-reachable origin in `s3.signer.uri`, not the bind
        // host (which defaults to 0.0.0.0 and is unreachable for engines).
        let public = config
            .catalog
            .public_url
            .clone()
            .unwrap_or_else(|| format!("http://localhost:{}", config.server.port));
        let base_url = format!("{}/catalog", public.trim_end_matches('/'));
        Some(egret_catalog::CatalogState {
            service,
            auth_token: config.catalog.auth_token.clone().into(),
            storage_cfg: std::sync::Arc::new(storage_cfg),
            base_url: base_url.into(),
        })
    } else {
        None
    };

    // Session-authenticated internal catalog browse API (for the UI). Always wired
    // (independent of `config.catalog.enabled`, which gates the Iceberg REST surface).
    let catalog_browse = Some(egret_catalog::BrowseState {
        store: egret_catalog::service::CatalogStore::new(db.clone()),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
    });

    // Hoist crypto/SecretStore/DatabaseRepo so both the database and engine blocks can share them.
    let databases = {
        use base64::Engine as _;
        let crypto = if config.secrets.master_key.is_empty() {
            None
        } else {
            let key = base64::engine::general_purpose::STANDARD
                .decode(config.secrets.master_key.as_bytes())
                .map_err(|e| anyhow::anyhow!("secrets.master_key base64: {e}"))?;
            Some(egret_secrets::Crypto::new(&key).map_err(|e| anyhow::anyhow!(e.to_string()))?)
        };
        let secrets = egret_secrets::SecretStore::new(db.clone(), crypto);
        egret_database::DatabaseRepo::new(db.clone(), secrets)
    };

    let database = Some(egret_database::DatabaseState {
        repo: databases.clone(),
        authz: Authorizer::new(db.clone()),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
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

    let app = build_router(AppState {
        db: Some(db),
        auth: Some(auth),
        workspace: Some(workspace),
        oidc,
        catalog,
        catalog_browse,
        ranger,
        database,
        engine,
        bi,
    });
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "egret listening");
    axum::serve(listener, app).await.context("server error")?;
    Ok(())
}
