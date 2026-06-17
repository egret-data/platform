//! Typed, layered configuration: built-in defaults → `egret.toml` → `EGRET_*` env vars.
//!
//! Nested keys use `__` as the env separator, e.g. `EGRET_SERVER__PORT=9000`.

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub log: LogConfig,
    pub session: SessionConfig,
    pub csrf: CsrfConfig,
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
}
