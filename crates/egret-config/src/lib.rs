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
                scopes: vec![
                    "openid".into(),
                    "email".into(),
                    "profile".into(),
                    "offline_access".into(),
                ],
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
    fn engine_preview_must_not_exceed_max_rows() {
        let mut cfg = Config::default();
        cfg.engine.preview_rows = 200_000; // > max_rows
        assert!(cfg.validate().is_err());
    }
}
