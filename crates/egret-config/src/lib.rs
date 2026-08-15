//! Typed, layered configuration: built-in defaults → `egret.toml` → `EGRET_*` env vars.
//!
//! Nested keys use `__` as the env separator, e.g. `EGRET_SERVER__PORT=9000`.

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub login_max_attempts: u32,
    pub login_window_secs: u64,
    pub login_lockout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub enabled: bool,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub post_login_redirect: String,
    /// Audience the catalog requires in an engine JWT's `aud` claim.
    pub catalog_audience: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// base64-encoded key (≥64 bytes) for encrypting the OIDC state cookie.
    pub secret_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangerSettings {
    pub enabled: bool,
    pub auth_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsConfig {
    /// base64-encoded master key (≥32 bytes) for AES-256-GCM credential encryption.
    /// Empty disables credential sealing — registering a database WITH a secret then fails.
    pub master_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub max_rows: usize,
    pub query_timeout_secs: u64,
    pub preview_rows: usize,
    /// Object-store key prefix for query results (under the storage bucket).
    pub results_prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSettings {
    pub enabled: bool,
    pub warehouse: String,
    pub auth_token: String,
    /// Externally-reachable base origin of this server (WITHOUT the `/catalog`
    /// suffix), advertised in `/v1/config`'s `s3.signer.uri`. Defaults to
    /// `http://localhost:8080`; set this to a client-reachable URL in production.
    pub public_url: Option<String>,
    /// Stable identity of the in-process "system" principal that egret uses to
    /// bootstrap lakekeeper's default project at startup. lakekeeper's
    /// `bootstrap` registers the calling user as the catalog's first
    /// (global-admin) user, so it must be a real, authenticated principal with a
    /// stable `UserId` — the in-process `LakekeeperInternal` actor has no
    /// `UserId` and is rejected with `MissingUserId`. This value is hashed into
    /// a deterministic lakekeeper `UserId` (`oidc~<system_user>`), so the same
    /// configured name yields the same admin principal on every boot. Keep it
    /// stable across deploys; changing it mints a *different* system admin.
    #[serde(default = "default_system_user")]
    pub system_user: String,
}

fn default_system_user() -> String {
    "egret-system".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSettings {
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub path_style: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelConfig {
    /// Which `KernelLauncher` impl to use: `"subprocess"` (default, local dev)
    /// or `"kubernetes"` (one pod per user×workspace; requires an in-cluster
    /// or kubeconfig-reachable API server).
    pub launcher: String,
    /// Namespace the `KubernetesLauncher` creates kernel Pods in.
    pub namespace: String,
    /// Kernel container image.
    pub image: String,
    pub image_pull_policy: String,
    pub cpu_request: String,
    pub mem_request: String,
    pub cpu_limit: String,
    pub mem_limit: String,
    /// Name of an existing `kubernetes.io/dockerconfigjson` Secret for pulling
    /// `image`, if the registry requires auth. `None` omits imagePullSecrets.
    pub pull_secret: Option<String>,
    /// Externally/cluster-reachable base URL kernels use to call back into
    /// egret (`EGRET_API_URL`). Overrides `catalog.public_url`-derived
    /// `api_url` for the kernel-env path only — set this when the server's
    /// own public URL isn't reachable from inside the cluster (e.g. behind an
    /// ingress the kernel pods can't resolve), such as a Service DNS name.
    pub callback_url: Option<String>,
    /// Accept the engine's self-signed TLS cert in kernel `trino()`/`starrocks()`
    /// — sets EGRET_ENGINE_TLS_INSECURE=1 on kernel pods. Dev/self-signed deploys
    /// only.
    pub engine_tls_insecure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// When false, the PDP is not wrapped and no events are emitted.
    pub enabled: bool,
    /// Rows older than this are pruned. Default 90.
    pub retention_days: i64,
    /// Bounded channel capacity; on overflow events are dropped (best-effort).
    pub channel_capacity: usize,
    /// Max events per INSERT batch.
    pub batch_size: usize,
    /// Flush the batch at least this often.
    pub flush_interval_ms: u64,
}

/// The Data API's serving surface (P4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataApiConfig {
    /// Rows in `dataapi.access_log` older than this are pruned hourly, beside
    /// the audit prune. Default 90, matching [`AuditConfig::retention_days`].
    ///
    /// # `0` means NEVER PRUNE here, and that is deliberately NOT
    /// [`AuditConfig::retention_days`]'s reading
    ///
    /// `AccessLogStore::prune(0)` — like `AuditStore::prune(0)` — deletes
    /// everything, because `ts < now() - 0 days` is every row already written.
    /// That is the right semantics for the STORE, which is a primitive an
    /// operator calls with an interval they chose.
    ///
    /// It is the wrong semantics for a CONFIG KEY, where `0` is what an operator
    /// writes when they mean "off" and what a half-written TOML or a
    /// `EGRET_DATA_API__ACCESS_LOG_RETENTION_DAYS=` env var can produce by
    /// accident. On the fastest-growing table in the system, that spelling would
    /// silently discard the whole log an hour later. So the scheduler treats `0`
    /// (and any negative) as "do not schedule the prune at all" and logs that it
    /// did — the store's meaning is untouched, and the trap is closed at the one
    /// place a human types a number.
    ///
    /// `AuditConfig::retention_days` still carries the trap unguarded. Fixing it
    /// is a behaviour change to another subsystem and is not being made here.
    pub access_log_retention_days: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub log: LogConfig,
    pub session: SessionConfig,
    pub csrf: CsrfConfig,
    pub rate_limit: RateLimitConfig,
    pub oidc: OidcConfig,
    pub security: SecurityConfig,
    pub catalog: CatalogSettings,
    pub storage: StorageSettings,
    pub ranger: RangerSettings,
    pub secrets: SecretsConfig,
    pub engine: EngineConfig,
    pub audit: AuditConfig,
    pub kernel: KernelConfig,
    pub automation: AutomationConfig,
    #[serde(default = "default_data_api")]
    pub data_api: DataApiConfig,
}

/// Serde's fallback for a `[data_api]` section a deployment's TOML predates.
///
/// `Config::figment()` layers `Serialized::defaults(Config::default())` under
/// the file, so a missing section is already covered — but only for callers
/// that go through `figment()`. Anything deserialising a `Config` directly
/// (a test, a future admin endpoint) would otherwise fail on a field that has
/// only ever had one sensible value.
fn default_data_api() -> DataApiConfig {
    DataApiConfig {
        access_log_retention_days: 90,
    }
}

/// Alerts & Reports (P2). The scheduler is on by default: a schedule the user
/// created and can see in the UI, which silently never runs, is worse than one
/// that runs and errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    /// Turn the ticker off entirely (a replica dedicated to serving, say).
    pub enabled: bool,
    /// Seconds between ticks. The floor on how promptly a due schedule runs;
    /// finer than the finest crontab (1 minute) buys nothing.
    pub tick_interval_secs: u64,
    /// Hosts a webhook may reach even though they resolve into private space.
    ///
    /// The escape hatch from the design's SSRF rule. It exists so that
    /// "webhook our own in-cluster incident bot" has a supported answer —
    /// without one, an operator who needs it turns the whole check off.
    pub webhook_allow_hosts: Vec<String>,
    /// Base URL of the `egret-renderer` service (P4), reachable from THIS
    /// server. In-cluster this is a Service DNS name, not a public hostname.
    pub renderer_url: String,
    /// Base URL the RENDERER can reach to fetch `/embed/print/{kind}/{id}` —
    /// in-cluster, this server's OWN Service name, which is generally not the
    /// same as the user-facing hostname (`catalog.public_url`) an ingress or a
    /// browser would use. Getting this wrong is a silent failure mode: the
    /// renderer's navigation would time out trying to reach an address only a
    /// browser outside the cluster can resolve.
    pub public_url: String,
    /// Base URL a PERSON can open, for the links in notifications.
    ///
    /// Deliberately separate from `public_url` above: that one is what the
    /// renderer resolves in-cluster, and a mail whose "open this" link points
    /// at a Service DNS name is a dead end in the recipient's inbox. Falls back
    /// to `public_url` when unset, which is correct in local dev where the two
    /// really are the same address.
    #[serde(default)]
    pub browser_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub cookie_name: String,
    pub absolute_ttl_hours: u32,
    pub idle_ttl_minutes: u32,
    pub secure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsrfConfig {
    pub cookie_name: String,
    pub header_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Postgres connection URL, e.g. `postgres://egret:egret@localhost:5432/egret`.
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// `tracing_subscriber` env-filter directive, e.g. `info,egret=debug`.
    pub filter: String,
    /// Emit JSON logs when true, pretty logs when false.
    pub json: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".into(),
                port: 8080,
            },
            database: DatabaseConfig {
                url: "postgres://egret:egret@localhost:5432/egret".into(),
                max_connections: 10,
            },
            log: LogConfig {
                filter: "info".into(),
                json: false,
            },
            session: SessionConfig {
                cookie_name: "egret_session".into(),
                absolute_ttl_hours: 24,
                idle_ttl_minutes: 60,
                secure: false,
            },
            csrf: CsrfConfig {
                cookie_name: "egret_csrf".into(),
                header_name: "X-Egret-CSRF".into(),
            },
            rate_limit: RateLimitConfig {
                login_max_attempts: 5,
                login_window_secs: 300,
                login_lockout_secs: 300,
            },
            oidc: OidcConfig {
                enabled: false,
                issuer_url: String::new(),
                client_id: String::new(),
                client_secret: String::new(),
                redirect_url: "http://localhost:8080/api/auth/oidc/callback".into(),
                // Groups come from the IdP's group protocol-mapper (emitted on the
                // `groups_claim`), so a `groups` *scope* is NOT requested by default —
                // Keycloak rejects an undefined scope with `invalid_scope`. Add "groups"
                // here only if your IdP exposes it as a client scope.
                scopes: vec!["openid".into(), "email".into(), "profile".into()],
                groups_claim: "groups".into(),
                post_login_redirect: "/".into(),
                catalog_audience: "egret-catalog".into(),
            },
            security: SecurityConfig {
                secret_key: String::new(),
            },
            catalog: CatalogSettings {
                enabled: false,
                warehouse: "s3://warehouse".into(),
                auth_token: String::new(),
                public_url: Some("http://localhost:8080".into()),
                system_user: default_system_user(),
            },
            storage: StorageSettings {
                endpoint: Some("http://localhost:9000".into()),
                region: "us-east-1".into(),
                bucket: "warehouse".into(),
                access_key: String::new(),
                secret_key: String::new(),
                path_style: true,
            },
            ranger: RangerSettings {
                enabled: false,
                auth_token: String::new(),
            },
            secrets: SecretsConfig {
                master_key: String::new(),
            },
            engine: EngineConfig {
                max_rows: 100_000,
                query_timeout_secs: 60,
                preview_rows: 100,
                results_prefix: "query-results".into(),
            },
            data_api: default_data_api(),
            audit: AuditConfig {
                enabled: true,
                retention_days: 90,
                channel_capacity: 8192,
                batch_size: 256,
                flush_interval_ms: 1000,
            },
            kernel: KernelConfig {
                launcher: "subprocess".into(),
                namespace: "egret-notebooks".into(),
                image: "egret-kernel:dev".into(),
                image_pull_policy: "IfNotPresent".into(),
                cpu_request: "250m".into(),
                mem_request: "512Mi".into(),
                cpu_limit: "1000m".into(),
                mem_limit: "2Gi".into(),
                pull_secret: None,
                callback_url: None,
                engine_tls_insecure: false,
            },
            automation: AutomationConfig {
                enabled: true,
                tick_interval_secs: 30,
                webhook_allow_hosts: vec![],
                renderer_url: "http://egret-renderer:8080".into(),
                public_url: "http://egret:8080".into(),
                browser_url: None,
            },
        }
    }
}

impl Config {
    /// Build the layered figment (defaults < file < env). Exposed for testing.
    pub fn figment() -> Figment {
        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file("egret.toml"))
            .merge(Env::prefixed("EGRET_").split("__"))
    }

    /// Load and validate configuration from all layers.
    #[allow(clippy::result_large_err)]
    pub fn load() -> Result<Self, figment::Error> {
        Self::figment().extract()
    }

    /// Validate cross-field invariants after loading. Call at boot.
    pub fn validate(&self) -> Result<(), String> {
        if self.oidc.enabled {
            use base64::Engine;
            let key_len = base64::engine::general_purpose::STANDARD
                .decode(self.security.secret_key.as_bytes())
                .map(|b| b.len())
                .unwrap_or(0);
            if key_len < 64 {
                return Err(
                    "oidc.enabled requires security.secret_key (base64, >=64 bytes)".into(),
                );
            }
            if self.oidc.issuer_url.is_empty() || self.oidc.client_id.is_empty() {
                return Err("oidc.enabled requires issuer_url and client_id".into());
            }
        }
        if self.catalog.enabled {
            if self.catalog.warehouse.is_empty() || self.catalog.auth_token.is_empty() {
                return Err("catalog.enabled requires warehouse and auth_token".into());
            }
            if self.storage.access_key.is_empty()
                || self.storage.secret_key.is_empty()
                || self.storage.bucket.is_empty()
            {
                return Err("catalog.enabled requires storage access_key/secret_key/bucket".into());
            }
        }
        // ranger.auth_token may be empty: that selects anonymous `/service` access for stock
        // Ranger plugins (e.g. Trino) that authenticate out-of-band. Set a token to require bearer.
        if !self.secrets.master_key.is_empty() {
            use base64::Engine;
            let key_len = base64::engine::general_purpose::STANDARD
                .decode(self.secrets.master_key.as_bytes())
                .map(|b| b.len())
                .unwrap_or(0);
            if key_len < 32 {
                return Err("secrets.master_key must be base64 of >=32 bytes".into());
            }
        }
        if self.engine.max_rows == 0 || self.engine.query_timeout_secs == 0 {
            return Err("engine.max_rows and engine.query_timeout_secs must be >= 1".into());
        }
        if self.engine.preview_rows > self.engine.max_rows {
            return Err("engine.preview_rows must be <= engine.max_rows".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_without_any_env() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.server.port, 8080);
            assert_eq!(cfg.server.host, "0.0.0.0");
            assert_eq!(cfg.database.max_connections, 10);
            Ok(())
        });
    }

    /// A fresh deployment must prune `dataapi.access_log`, and must not prune it
    /// to nothing.
    ///
    /// The two halves are separate assertions on purpose. A default of 0 would
    /// deserialise fine, read as "off" through `scheduled_retention`, and leave
    /// the fastest-growing table in the system unbounded on every deployment
    /// that never set the key — which is every deployment today, since the
    /// `[data_api]` section is new.
    #[test]
    fn the_access_log_retention_default_is_positive() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert!(
                cfg.data_api.access_log_retention_days > 0,
                "a fresh deployment must prune the access log, got {}",
                cfg.data_api.access_log_retention_days
            );
            assert_eq!(
                cfg.data_api.access_log_retention_days, cfg.audit.retention_days,
                "the two retention knobs should not disagree by accident"
            );
            Ok(())
        });
    }

    /// The key is reachable from the environment under the name the operator
    /// docs will name. A section whose env path is wrong is a knob that silently
    /// cannot be turned.
    #[test]
    fn the_access_log_retention_is_settable_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_DATA_API__ACCESS_LOG_RETENTION_DAYS", "7");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.data_api.access_log_retention_days, 7);
            Ok(())
        });
    }

    #[test]
    fn env_overrides_nested_keys() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_SERVER__PORT", "9999");
            jail.set_env("EGRET_DATABASE__URL", "postgres://x/y");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.server.port, 9999);
            assert_eq!(cfg.database.url, "postgres://x/y");
            Ok(())
        });
    }

    #[test]
    fn toml_file_is_merged() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("egret.toml", "[server]\nport = 7000\n")?;
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.server.port, 7000);
            Ok(())
        });
    }

    #[test]
    fn session_and_csrf_defaults_present() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.session.absolute_ttl_hours, 24);
            assert_eq!(cfg.session.idle_ttl_minutes, 60);
            assert_eq!(cfg.csrf.header_name, "X-Egret-CSRF");
            Ok(())
        });
    }

    #[test]
    fn rate_limit_defaults_present() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.rate_limit.login_max_attempts, 5);
            Ok(())
        });
    }

    #[test]
    fn validate_ok_when_oidc_disabled() {
        let cfg = Config::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_requires_secret_key_when_oidc_enabled() {
        let mut cfg = Config::default();
        cfg.oidc.enabled = true;
        cfg.oidc.issuer_url = "http://idp".into();
        cfg.oidc.client_id = "egret".into();
        assert!(cfg.validate().is_err()); // empty secret_key
    }

    #[test]
    fn validate_ok_for_dev_oidc_config() {
        // Mirrors the dev `.env` / `.env.example`: OIDC on against local Keycloak,
        // with a dev secret_key (base64 of 64 random bytes). Must match .env.example.
        let mut cfg = Config::default();
        cfg.oidc.enabled = true;
        cfg.oidc.issuer_url = "http://localhost:8081/realms/egret".into();
        cfg.oidc.client_id = "egret".into();
        cfg.oidc.client_secret = "egret-secret".into();
        cfg.oidc.redirect_url = "http://localhost:5173/api/auth/oidc/callback".into();
        cfg.security.secret_key =
            "Vi2XPRkbfyTLbSzhWv0VBYeOozGGoUnVSL3pIx2Oy1kbBphJyL0CXpomCNKdrYndzYnOX8MH0oaxhcY6Bt/x8g=="
                .into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn catalog_disabled_by_default_validates() {
        let cfg = Config::default();
        assert!(!cfg.catalog.enabled);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn catalog_enabled_requires_token_and_creds() {
        let mut cfg = Config::default();
        cfg.catalog.enabled = true;
        assert!(cfg.validate().is_err());
        cfg.catalog.auth_token = "t".into();
        cfg.storage.access_key = "a".into();
        cfg.storage.secret_key = "s".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ranger_disabled_by_default_validates() {
        let cfg = Config::default();
        assert!(!cfg.ranger.enabled);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ranger_enabled_validates_with_or_without_token() {
        let mut cfg = Config::default();
        cfg.ranger.enabled = true;
        // Empty token = anonymous /service (for stock Ranger plugins). Valid.
        assert!(cfg.validate().is_ok());
        // A configured token is also valid (requires bearer).
        cfg.ranger.auth_token = "t".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn secrets_master_key_defaults_empty_and_validates() {
        let cfg = Config::default();
        assert!(cfg.secrets.master_key.is_empty());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn secrets_master_key_must_be_base64_32_bytes_when_set() {
        let mut cfg = Config::default();
        cfg.secrets.master_key = "tooshort".into(); // not valid base64 of >=32 bytes
        assert!(cfg.validate().is_err());
        use base64::Engine;
        cfg.secrets.master_key = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn engine_defaults_present_and_validate() {
        let cfg = Config::default();
        assert_eq!(cfg.engine.max_rows, 100_000);
        assert_eq!(cfg.engine.query_timeout_secs, 60);
        assert_eq!(cfg.engine.preview_rows, 100);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn audit_defaults_present() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert!(cfg.audit.enabled);
            assert_eq!(cfg.audit.retention_days, 90);
            assert_eq!(cfg.audit.channel_capacity, 8192);
            assert_eq!(cfg.audit.batch_size, 256);
            assert_eq!(cfg.audit.flush_interval_ms, 1000);
            Ok(())
        });
    }

    #[test]
    fn engine_preview_must_not_exceed_max_rows() {
        let mut cfg = Config::default();
        cfg.engine.preview_rows = 200_000; // > max_rows
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn kernel_defaults_to_subprocess_launcher() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.kernel.launcher, "subprocess");
            assert_eq!(cfg.kernel.namespace, "egret-notebooks");
            assert!(cfg.kernel.pull_secret.is_none());
            assert!(cfg.kernel.callback_url.is_none());
            Ok(())
        });
    }

    #[test]
    fn kernel_launcher_selectable_via_env() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_KERNEL__LAUNCHER", "kubernetes");
            jail.set_env("EGRET_KERNEL__NAMESPACE", "custom-ns");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.kernel.launcher, "kubernetes");
            assert_eq!(cfg.kernel.namespace, "custom-ns");
            Ok(())
        });
    }
}
